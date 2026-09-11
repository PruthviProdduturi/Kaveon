//! Exact metadata statistics bound to an immutable source identity.

use crate::{IcebergReader, ObjectDeltaReader, ObjectLocation, ObjectParquetReader, ParquetReader};
use kaveon_core::{DataFormat, KaveonError, Result};
use sha2::{Digest, Sha256};
use std::{fs, time::UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStatistics {
    pub identity_sha256: String,
    pub row_count: u64,
    pub columns: Vec<String>,
}

/// Resolves the source and derives exact row count without reading data pages.
/// Calling this again after analysis is the compare step which prevents stale
/// statistics from being published after a concurrent table update.
pub fn analyze_source(location: &str, format: DataFormat) -> Result<SourceStatistics> {
    match (is_object(location), format) {
        (true, DataFormat::Delta) => {
            let snapshot = ObjectDeltaReader::from_uri(location)?.snapshot()?;
            let metadata = ObjectDeltaReader::from_uri(location)?
                .with_version(snapshot.version)
                .metadata()?;
            let mut identity = format!("delta\n{}\n{}\n", location, snapshot.version);
            for file in snapshot.files {
                identity.push_str(file.as_ref());
                identity.push('\n');
            }
            Ok(stats(identity, metadata.row_count, &metadata.schema))
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
            let parquet = crate::delta_snapshot::blocking(async move {
                ObjectParquetReader::new(object.store, object.path)
                    .metadata()
                    .await
            })?;
            Ok(stats(
                format!("parquet\n{location}\n{version}\n{}", meta.size),
                parquet.row_count,
                &parquet.schema,
            ))
        }
        (false, DataFormat::Parquet) => {
            let file = fs::metadata(location)?;
            let modified = file
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|_| KaveonError::Storage("file modification time precedes epoch".into()))?
                .as_nanos();
            let parquet = ParquetReader::new(location).metadata()?;
            Ok(stats(
                format!("parquet-local\n{location}\n{}\n{modified}", file.len()),
                parquet.row_count,
                &parquet.schema,
            ))
        }
        (false, DataFormat::Delta) => {
            let version = crate::DeltaTableReader::new(location).snapshot_version()?;
            let metadata = crate::DeltaTableReader::new(location)
                .with_version(version)
                .metadata()?;
            let identity = format!("delta-local\n{location}\n{version}");
            Ok(stats(identity, metadata.row_count, &metadata.schema))
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
            })
        }
    }
}

fn stats(
    identity: String,
    row_count: u64,
    schema: &arrow::datatypes::SchemaRef,
) -> SourceStatistics {
    SourceStatistics {
        identity_sha256: digest(identity),
        row_count,
        columns: schema.fields().iter().map(|f| f.name().clone()).collect(),
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

        std::fs::write(
            log.join("00000000000000000001.json"),
            "{\"remove\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"third.parquet\"}}",
        )
        .unwrap();
        let second = analyze_source(directory.to_str().unwrap(), DataFormat::Delta).unwrap();
        assert_eq!(second.row_count, 12);
        assert_ne!(first.identity_sha256, second.identity_sha256);

        std::fs::remove_dir_all(directory).unwrap();
    }
}
