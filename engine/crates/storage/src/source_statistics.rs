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
