//! Exact metadata statistics bound to an immutable source identity.

use crate::{IcebergReader, ObjectDeltaReader, ObjectLocation, ObjectParquetReader, ParquetReader};
use kaveon_core::{DataFormat, KaveonError, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};

const MAX_EXACT_STATISTICS_CACHE_ENTRIES: usize = 256;
static EXACT_STATISTICS_CACHE: OnceLock<Mutex<HashMap<String, SourceStatistics>>> = OnceLock::new();
static LATEST_DELTA_STATISTICS: OnceLock<Mutex<HashMap<String, (u64, SourceStatistics)>>> =
    OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStatistics {
    pub identity_sha256: String,
    pub row_count: u64,
    pub columns: Vec<String>,
    /// Immutable Delta version used to derive these statistics.
    pub delta_version: Option<u64>,
}

/// Resolves the source and derives exact row count without reading data pages.
/// Calling this again after analysis is the compare step which prevents stale
/// statistics from being published after a concurrent table update.
pub fn analyze_source(location: &str, format: DataFormat) -> Result<SourceStatistics> {
    match (is_object(location), format) {
        (true, DataFormat::Delta) => {
            let reader = ObjectDeltaReader::from_uri(location)?;
            analyze_object_delta(location, &reader)
        }
        (true, DataFormat::Iceberg) => {
            let snapshot = IcebergReader::new(location).snapshot()?;
            let mut identity = format!(
                "iceberg\n{}\n{:?}\n",
                snapshot.metadata_uri, snapshot.snapshot_id
            );
            for file in &snapshot.files {
                identity.push_str(file);
                identity.push('\n');
            }
            Ok(SourceStatistics {
                identity_sha256: digest(identity),
                row_count: snapshot.row_count,
                columns: snapshot
                    .schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect(),
                delta_version: None,
            })
        }
        (true, DataFormat::Parquet) => {
            let object = ObjectLocation::from_uri(location)?;
            let meta = crate::delta_snapshot::blocking({
                let store = object.store.clone();
                let path = object.path.clone();
                async move {
                    store
                        .head(&path)
                        .await
                        .map_err(|e| KaveonError::Storage(e.to_string()))
                }
            })?;
            let version = meta.e_tag.or(meta.version).ok_or_else(|| {
                KaveonError::Storage(
                    "object store did not provide an ETag or version for stable ANALYZE".into(),
                )
            })?;
            let identity_sha256 = digest(format!("parquet\n{location}\n{version}\n{}", meta.size));
            if let Some(cached) = cached_statistics(&identity_sha256) {
                return Ok(cached);
            }
            let parquet = crate::delta_snapshot::blocking(async move {
                ObjectParquetReader::new(object.store, object.path)
                    .metadata()
                    .await
            })?;
            Ok(cache_statistics(stats_from_digest(
                identity_sha256,
                parquet.row_count,
                &parquet.schema,
            )))
        }
        (false, DataFormat::Parquet) => {
            let file = fs::metadata(location)?;
            let modified = file
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|_| KaveonError::Storage("file modification time precedes epoch".into()))?
                .as_nanos();
            let identity_sha256 = digest(format!(
                "parquet-local\n{location}\n{}\n{modified}",
                file.len()
            ));
            if let Some(cached) = cached_statistics(&identity_sha256) {
                return Ok(cached);
            }
            let parquet = ParquetReader::new(location).metadata()?;
            Ok(cache_statistics(stats_from_digest(
                identity_sha256,
                parquet.row_count,
                &parquet.schema,
            )))
        }
        (false, DataFormat::Delta) => {
            let version = crate::DeltaTableReader::new(location).snapshot_version()?;
            let identity_sha256 = digest(format!("delta-local\n{location}\n{version}"));
            if let Some(cached) = cached_statistics(&identity_sha256) {
                return Ok(cached);
            }
            let metadata = crate::DeltaTableReader::new(location)
                .with_version(version)
                .metadata()?;
            let mut statistics =
                stats_from_digest(identity_sha256, metadata.row_count, &metadata.schema);
            statistics.delta_version = Some(version);
            Ok(cache_statistics(statistics))
        }
        (false, DataFormat::Iceberg) => {
            let snapshot = IcebergReader::new(location).snapshot()?;
            Ok(SourceStatistics {
                identity_sha256: digest(format!(
                    "iceberg-local\n{}\n{:?}\n{:?}",
                    snapshot.metadata_uri, snapshot.snapshot_id, snapshot.files
                )),
                row_count: snapshot.row_count,
                columns: snapshot
                    .schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect(),
                delta_version: None,
            })
        }
    }
}

fn analyze_object_delta(location: &str, reader: &ObjectDeltaReader) -> Result<SourceStatistics> {
    if let Some((version, statistics)) = cached_delta_statistics(location)
        && reader.is_latest_version(version)?
    {
        return Ok(statistics);
    }
    let snapshot = reader.snapshot()?;
    let version = snapshot.version;
    let mut identity = format!("delta\n{}\n{}\n", location, snapshot.version);
    for file in &snapshot.files {
        identity.push_str(file.as_ref());
        identity.push('\n');
    }
    let identity_sha256 = digest(identity);
    if let Some(cached) = cached_statistics(&identity_sha256) {
        cache_delta_statistics(location, version, cached.clone());
        return Ok(cached);
    }
    let metadata = reader.metadata_for_snapshot(snapshot)?;
    let statistics = cache_statistics(SourceStatistics {
        identity_sha256,
        row_count: metadata.row_count,
        columns: metadata
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect(),
        delta_version: Some(version),
    });
    cache_delta_statistics(location, version, statistics.clone());
    Ok(statistics)
}

fn stats_from_digest(
    identity_sha256: String,
    row_count: u64,
    schema: &arrow::datatypes::SchemaRef,
) -> SourceStatistics {
    SourceStatistics {
        identity_sha256,
        row_count,
        columns: schema.fields().iter().map(|f| f.name().clone()).collect(),
        delta_version: None,
    }
}

fn cached_statistics(identity_sha256: &str) -> Option<SourceStatistics> {
    EXACT_STATISTICS_CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(identity_sha256)
        .cloned()
}

fn cache_statistics(statistics: SourceStatistics) -> SourceStatistics {
    let Ok(mut cache) = EXACT_STATISTICS_CACHE.get_or_init(Default::default).lock() else {
        return statistics;
    };
    if cache.len() >= MAX_EXACT_STATISTICS_CACHE_ENTRIES
        && !cache.contains_key(&statistics.identity_sha256)
    {
        // Statistics are an optimization only. Clearing at the fixed bound is
        // deterministic, keeps memory bounded, and cannot affect correctness.
        cache.clear();
    }
    cache.insert(statistics.identity_sha256.clone(), statistics.clone());
    statistics
}

fn cached_delta_statistics(location: &str) -> Option<(u64, SourceStatistics)> {
    LATEST_DELTA_STATISTICS
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(location)
        .cloned()
}

fn cache_delta_statistics(location: &str, version: u64, statistics: SourceStatistics) {
    let Ok(mut cache) = LATEST_DELTA_STATISTICS.get_or_init(Default::default).lock() else {
        return;
    };
    if cache.len() >= MAX_EXACT_STATISTICS_CACHE_ENTRIES && !cache.contains_key(location) {
        cache.clear();
    }
    // Concurrent analyses may finish out of order. Never let an older snapshot
    // replace a newer cache entry.
    if cache
        .get(location)
        .is_none_or(|(cached, _)| version >= *cached)
    {
        cache.insert(location.to_owned(), (version, statistics));
    }
}
fn digest(value: String) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn is_object(value: &str) -> bool {
    value.starts_with("abfss://") || value.starts_with("s3://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use object_store::{ObjectStore, memory::InMemory, path::Path};
    use parquet::arrow::ArrowWriter;
    use std::{fs::File, sync::Arc};

    fn write(path: &std::path::Path, rows: usize) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values((0..rows).map(|v| v as i64))) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|v| format!("r{v}")),
                )) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(File::create(path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    fn parquet_bytes(rows: usize) -> Vec<u8> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values((0..rows).map(|v| v as i64))) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|v| format!("r{v}")),
                )) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(Vec::new(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    #[test]
    fn local_parquet_statistics_are_exact_stable_and_change_on_replacement() {
        let path =
            std::env::temp_dir().join(format!("kaveon-analyze-{}.parquet", std::process::id()));
        write(&path, 3);
        let first = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        let repeated = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(first.row_count, 3);
        assert_eq!(first.columns, ["id", "name"]);
        assert_eq!(first.identity_sha256, repeated.identity_sha256);
        write(&path, 17);
        let replaced = analyze_source(path.to_str().unwrap(), DataFormat::Parquet).unwrap();
        assert_eq!(replaced.row_count, 17);
        assert_ne!(first.identity_sha256, replaced.identity_sha256);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn local_delta_statistics_follow_the_exact_active_snapshot() {
        let directory =
            std::env::temp_dir().join(format!("kaveon-analyze-delta-{}", std::process::id()));
        let log = directory.join("_delta_log");
        std::fs::create_dir_all(&log).unwrap();
        write(&directory.join("first.parquet"), 3);
        write(&directory.join("second.parquet"), 5);
        write(&directory.join("third.parquet"), 7);
        std::fs::write(
            log.join("00000000000000000000.json"),
            "{\"add\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"second.parquet\"}}",
        )
        .unwrap();

        let first = analyze_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(first.row_count, 8);
        assert_eq!(first.columns, ["id", "name"]);
        assert_eq!(first.delta_version, Some(0));

        std::fs::write(
            log.join("00000000000000000001.json"),
            "{\"remove\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"third.parquet\"}}",
        )
        .unwrap();
        let second = analyze_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(second.row_count, 12);
        assert_ne!(first.identity_sha256, second.identity_sha256);
        assert_eq!(second.delta_version, Some(1));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn object_delta_cached_statistics_invalidate_on_next_commit() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (name, rows) in [("first.parquet", 3), ("second.parquet", 7)] {
            runtime
                .block_on(store.put(
                    &Path::from(format!("table/{name}")),
                    parquet_bytes(rows).into(),
                ))
                .unwrap();
        }
        runtime
            .block_on(store.put(
                &Path::from("table/_delta_log/00000000000000000000.json"),
                br#"{"add":{"path":"first.parquet"}}"#.to_vec().into(),
            ))
            .unwrap();
        let reader = ObjectDeltaReader::new(store.clone(), Path::from("table"));
        let location = format!("memory://delta-cache-{}", std::process::id());
        let first = analyze_object_delta(&location, &reader).unwrap();
        assert_eq!(first.row_count, 3);
        assert_eq!(first.delta_version, Some(0));
        assert_eq!(analyze_object_delta(&location, &reader).unwrap(), first);

        runtime
            .block_on(
                store.put(
                    &Path::from("table/_delta_log/00000000000000000001.json"),
                    br#"{"remove":{"path":"first.parquet"}}
{"add":{"path":"second.parquet"}}"#
                        .to_vec()
                        .into(),
                ),
            )
            .unwrap();
        let second = analyze_object_delta(&location, &reader).unwrap();
        assert_eq!(second.row_count, 7);
        assert_ne!(second.identity_sha256, first.identity_sha256);
        assert_eq!(second.delta_version, Some(1));
    }
}
