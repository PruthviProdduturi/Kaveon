use crate::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader, ScanMetrics, ScanPartition};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchSource, KaveonError, Result, StoragePredicate};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_BATCH_SIZE: usize = 8_192;
#[cfg(test)]
const DELTA_VERSION_WIDTH: usize = 20;

/// A synchronous reader for a local Delta Lake table backed by Parquet files.
///
/// The reader replays JSON transaction-log actions to select the active files in
/// the latest snapshot. It deliberately rejects incomplete log histories rather
/// than returning a partial table.
pub struct DeltaTableReader {
    path: PathBuf,
    version: Option<u64>,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
}

impl DeltaTableReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            version: None,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
        }
    }

    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    pub fn snapshot_version(&self) -> Result<u64> {
        Ok(local_snapshot(&self.path, self.version)?.version)
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    pub fn with_partition(mut self, partition: ScanPartition) -> Self {
        self.partition = Some(partition);
        self
    }

    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(predicate);
        self
    }

    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        let snapshot = local_snapshot(&self.path, self.version)?;
        let files = snapshot
            .files
            .iter()
            .map(|path| self.path.join(path.as_ref()))
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(ParquetFileMetadata {
                schema: snapshot
                    .schema
                    .ok_or_else(|| delta_error("empty Delta snapshot has no logical schema"))?,
                row_count: 0,
                row_group_count: 0,
            });
        }
        let first = files
            .first()
            .ok_or_else(|| delta_error("Delta snapshot has no active files"))?;
        let mut metadata = ParquetReader::new(first).metadata()?;
        for path in files.iter().skip(1) {
            let next = ParquetReader::new(path).metadata()?;
            if next.schema != metadata.schema {
                return Err(delta_error(format!(
                    "Delta snapshot contains incompatible Parquet schema in '{}'",
                    path.display()
                )));
            }
            metadata.row_count = metadata
                .row_count
                .checked_add(next.row_count)
                .ok_or_else(|| delta_error("Delta row count overflow"))?;
            metadata.row_group_count = metadata
                .row_group_count
                .saturating_add(next.row_group_count);
        }
        if let Some(schema) = snapshot.schema {
            crate::delta_snapshot::validate_physical_schema(&schema, &metadata.schema)?;
            metadata.schema = schema;
        }
        Ok(metadata)
    }

    /// Resolves the latest local snapshot into its deterministic active-file order.
    ///
    /// This is intentionally metadata-only so coordinators can create distributed
    /// scan splits without opening or decoding every data file.
    pub fn active_file_paths(&self) -> Result<Vec<PathBuf>> {
        active_files_at(&self.path, self.version)
    }

    pub fn read(&self) -> Result<DeltaBatchIterator> {
        if self.batch_size == 0 {
            return Err(delta_error("batch size must be greater than zero"));
        }
        let snapshot_started = Instant::now();
        let snapshot = local_snapshot(&self.path, self.version)?;
        let all_files = snapshot
            .files
            .iter()
            .map(|path| self.path.join(path.as_ref()))
            .collect::<Vec<_>>();
        let snapshot_elapsed = snapshot_started.elapsed();
        if all_files.is_empty() {
            let schema = projected_schema(
                snapshot
                    .schema
                    .ok_or_else(|| delta_error("empty Delta snapshot has no logical schema"))?,
                self.columns.as_deref(),
            )?;
            return Ok(DeltaBatchIterator {
                files: vec![],
                next_file: 0,
                current: None,
                schema,
                batch_size: self.batch_size,
                columns: self.columns.clone(),
                predicate: self.predicate.clone(),
                metrics: ScanMetrics::default(),
            });
        }
        let schema_file = all_files
            .first()
            .cloned()
            .ok_or_else(|| delta_error("Delta snapshot has no active files"))?;
        let files: Vec<_> = all_files
            .into_iter()
            .enumerate()
            .filter_map(|(index, path)| {
                self.partition
                    .is_none_or(|partition| partition.contains(index))
                    .then_some(path)
            })
            .collect();
        let metrics = ScanMetrics::default();
        metrics.snapshot_time(snapshot_elapsed);
        let current = files
            .first()
            .map(|first| {
                configured_reader(
                    first,
                    self.batch_size,
                    self.columns.as_ref(),
                    self.predicate.as_ref(),
                    metrics.clone(),
                )
                .read()
            })
            .transpose()?;
        let mut schema = match current.as_ref() {
            Some(reader) => reader.schema().clone(),
            None => projected_schema(
                configured_reader(
                    &schema_file,
                    self.batch_size,
                    None,
                    None,
                    ScanMetrics::default(),
                )
                .metadata()?
                .schema,
                self.columns.as_deref(),
            )?,
        };
        if let Some(logical_schema) = snapshot.schema {
            let physical_schema = ParquetReader::new(&schema_file).metadata()?.schema;
            crate::delta_snapshot::validate_physical_schema(&logical_schema, &physical_schema)?;
            schema = projected_schema(logical_schema, self.columns.as_deref())?;
        }
        Ok(DeltaBatchIterator {
            files,
            next_file: usize::from(current.is_some()),
            current,
            schema,
            batch_size: self.batch_size,
            columns: self.columns.clone(),
            predicate: self.predicate.clone(),
            metrics,
        })
    }
}

fn projected_schema(schema: SchemaRef, columns: Option<&[String]>) -> Result<SchemaRef> {
    let Some(columns) = columns else {
        return Ok(schema);
    };
    let indices = crate::parquet_reader::projection_indices(&schema, columns)?;
    Ok(std::sync::Arc::new(schema.project(&indices)?))
}

pub struct DeltaBatchIterator {
    files: Vec<PathBuf>,
    next_file: usize,
    current: Option<ParquetBatchIterator>,
    schema: SchemaRef,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    metrics: ScanMetrics,
}

impl BatchSource for DeltaBatchIterator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(reader) = self.current.as_mut()
                && let Some(batch) = reader.next_batch()?
            {
                crate::delta_snapshot::validate_physical_schema(&self.schema, &batch.schema())?;
                return Ok(Some(RecordBatch::try_new(
                    self.schema.clone(),
                    batch.columns().to_vec(),
                )?));
            }
            let Some(path) = self.files.get(self.next_file) else {
                return Ok(None);
            };
            self.current = Some(
                configured_reader(
                    path,
                    self.batch_size,
                    self.columns.as_ref(),
                    self.predicate.as_ref(),
                    self.metrics.clone(),
                )
                .read()?,
            );
            self.next_file += 1;
        }
    }
}

impl DeltaBatchIterator {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

fn configured_reader(
    path: &Path,
    batch_size: usize,
    columns: Option<&Vec<String>>,
    predicate: Option<&StoragePredicate>,
    metrics: ScanMetrics,
) -> ParquetReader {
    let mut reader = ParquetReader::new(path)
        .with_batch_size(batch_size)
        .with_metrics(metrics);
    if let Some(predicate) = predicate {
        reader = reader.with_predicate(predicate.clone());
    }
    match columns {
        Some(columns) => reader.with_columns(columns.clone()),
        None => reader,
    }
}

fn local_snapshot(
    table_path: &Path,
    version: Option<u64>,
) -> Result<crate::delta_snapshot::DeltaSnapshot> {
    let table = table_path.to_path_buf();
    let store = std::sync::Arc::new(
        object_store::local::LocalFileSystem::new_with_prefix(&table)
            .map_err(crate::object_reader::storage_error)?,
    );
    crate::delta_snapshot::blocking(async move {
        crate::delta_snapshot::resolve_snapshot(
            store,
            &object_store::path::Path::default(),
            version,
        )
        .await
    })
}

fn active_files_at(table_path: &Path, version: Option<u64>) -> Result<Vec<PathBuf>> {
    Ok(local_snapshot(table_path, version)?
        .files
        .into_iter()
        .map(|path| table_path.join(path.as_ref()))
        .collect())
}

#[cfg(test)]
fn active_files(table_path: &Path) -> Result<Vec<PathBuf>> {
    active_files_at(table_path, None)
}

fn delta_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIRECTORY_SEQUENCE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    fn test_table() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kaveon-delta-{}-{unique}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("_delta_log")).expect("test Delta log should be created");
        path
    }

    fn write_commit(table: &Path, version: usize, actions: &str) {
        let name = format!("{version:0DELTA_VERSION_WIDTH$}.json");
        fs::write(table.join("_delta_log").join(name), actions)
            .expect("test Delta commit should be written");
    }

    #[test]
    fn predicate_prunes_row_groups_and_filters_rows_across_all_files() {
        use arrow::{
            array::Int64Array,
            datatypes::{DataType, Field, Schema},
        };
        use kaveon_core::{CompareOp, ScalarValue};
        use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
        use std::sync::Arc;
        let table = test_table();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("payload", DataType::Int64, false),
        ]));
        for file in 0..2 {
            let values = (file * 8..file * 8 + 8).collect::<Vec<i64>>();
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(values.clone())),
                    Arc::new(Int64Array::from(values)),
                ],
            )
            .unwrap();
            let mut writer = ArrowWriter::try_new(
                fs::File::create(table.join(format!("part-{file}.parquet"))).unwrap(),
                schema.clone(),
                Some(
                    WriterProperties::builder()
                        .set_max_row_group_size(4)
                        .build(),
                ),
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }
        write_commit(
            &table,
            0,
            "{\"add\":{\"path\":\"part-0.parquet\"}}\n{\"add\":{\"path\":\"part-1.parquet\"}}",
        );
        let mut source = DeltaTableReader::new(&table)
            .with_columns(vec!["payload".into()])
            .with_predicate(StoragePredicate::Compare {
                column: "id".into(),
                op: CompareOp::Lt,
                value: ScalarValue::Int64(2),
            })
            .read()
            .unwrap();
        let mut values = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            values.extend(
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied(),
            );
        }
        assert_eq!(values, vec![0, 1]);
        assert_eq!(source.metrics().snapshot().row_groups_pruned(), 3);
        fs::remove_dir_all(table).unwrap();
    }

    #[test]
    fn empty_snapshot_preserves_declared_schema_and_projection() {
        let table = test_table();
        let schema = serde_json::json!({"type":"struct","fields":[{"name":"id","type":"long","nullable":true},{"name":"amount","type":"decimal(20,4)","nullable":true}]}).to_string();
        write_commit(
            &table,
            0,
            &serde_json::json!({"metaData":{"schemaString":schema,"partitionColumns":[]}})
                .to_string(),
        );
        let metadata = DeltaTableReader::new(&table).metadata().unwrap();
        assert_eq!(metadata.row_count, 0);
        assert_eq!(
            metadata.schema.field(1).data_type(),
            &arrow::datatypes::DataType::Decimal128(20, 4)
        );
        let mut source = DeltaTableReader::new(&table)
            .with_columns(vec!["amount".into(), "id".into()])
            .read()
            .unwrap();
        assert_eq!(source.schema().field(0).name(), "amount");
        assert!(source.next_batch().unwrap().is_none());
        assert!(
            DeltaTableReader::new(&table)
                .with_columns(vec!["missing".into()])
                .read()
                .is_err()
        );
        fs::remove_dir_all(&table).unwrap();
    }

    #[test]
    fn snapshot_applies_add_and_remove_actions() {
        let table = test_table();
        write_commit(
            &table,
            0,
            "{\"add\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"second.parquet\"}}",
        );
        write_commit(
            &table,
            1,
            "{\"remove\":{\"path\":\"first.parquet\"}}\n{\"add\":{\"path\":\"third.parquet\"}}",
        );

        let files = active_files(&table).expect("snapshot should resolve");
        assert_eq!(
            files,
            vec![table.join("second.parquet"), table.join("third.parquet")]
        );
        fs::remove_dir_all(&table).expect("test table should be removed");
    }

    #[test]
    fn snapshot_rejects_incomplete_json_history() {
        let table = test_table();
        write_commit(&table, 1, "{\"add\":{\"path\":\"part.parquet\"}}");

        let error = active_files(&table).expect_err("incomplete history should fail");
        assert!(error.to_string().contains("incomplete at version 0"));
        fs::remove_dir_all(&table).expect("test table should be removed");
    }

    #[test]
    fn snapshot_rejects_parent_directory_paths() {
        let table = test_table();
        write_commit(&table, 0, "{\"add\":{\"path\":\"../outside.parquet\"}}");

        let error = active_files(&table).expect_err("unsafe path should fail");
        assert!(error.to_string().contains("relative path"));
        fs::remove_dir_all(&table).expect("test table should be removed");
    }
}
