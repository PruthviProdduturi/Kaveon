use crate::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader, ScanMetrics, ScanPartition};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchSource, KaveonError, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

const DEFAULT_BATCH_SIZE: usize = 8_192;
const DELTA_VERSION_WIDTH: usize = 20;

/// A synchronous reader for a local Delta Lake table backed by Parquet files.
///
/// The reader replays JSON transaction-log actions to select the active files in
/// the latest snapshot. It deliberately rejects incomplete log histories rather
/// than returning a partial table.
pub struct DeltaTableReader {
    path: PathBuf,
    batch_size: usize,
    columns: Option<Vec<String>>,
    partition: Option<ScanPartition>,
}

impl DeltaTableReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            partition: None,
        }
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

    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        let files = active_files(&self.path)?;
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
            metadata.row_count = metadata.row_count.saturating_add(next.row_count);
            metadata.row_group_count = metadata
                .row_group_count
                .saturating_add(next.row_group_count);
        }
        Ok(metadata)
    }

    /// Resolves the latest local snapshot into its deterministic active-file order.
    ///
    /// This is intentionally metadata-only so coordinators can create distributed
    /// scan splits without opening or decoding every data file.
    pub fn active_file_paths(&self) -> Result<Vec<PathBuf>> {
        active_files(&self.path)
    }

    pub fn read(&self) -> Result<DeltaBatchIterator> {
        if self.batch_size == 0 {
            return Err(delta_error("batch size must be greater than zero"));
        }
        let snapshot_started = Instant::now();
        let all_files = active_files(&self.path)?;
        let snapshot_elapsed = snapshot_started.elapsed();
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
                    metrics.clone(),
                )
                .read()
            })
            .transpose()?;
        let schema = match current.as_ref() {
            Some(reader) => reader.schema().clone(),
            None => projected_schema(
                configured_reader(&schema_file, self.batch_size, None, ScanMetrics::default())
                    .metadata()?
                    .schema,
                self.columns.as_deref(),
            )?,
        };
        Ok(DeltaBatchIterator {
            files,
            next_file: usize::from(current.is_some()),
            current,
            schema,
            batch_size: self.batch_size,
            columns: self.columns.clone(),
            metrics,
        })
    }
}

fn projected_schema(schema: SchemaRef, columns: Option<&[String]>) -> Result<SchemaRef> {
    let Some(columns) = columns else {
        return Ok(schema);
    };
    let fields = columns
        .iter()
        .map(|column| {
            schema.field_with_name(column).cloned().map_err(|_| {
                delta_error(format!("projection references unknown column '{column}'"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(std::sync::Arc::new(arrow::datatypes::Schema::new(fields)))
}

pub struct DeltaBatchIterator {
    files: Vec<PathBuf>,
    next_file: usize,
    current: Option<ParquetBatchIterator>,
    schema: SchemaRef,
    batch_size: usize,
    columns: Option<Vec<String>>,
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
                if batch.schema() != self.schema {
                    return Err(delta_error(
                        "Delta snapshot contains incompatible Parquet schemas",
                    ));
                }
                return Ok(Some(batch));
            }
            let Some(path) = self.files.get(self.next_file) else {
                return Ok(None);
            };
            self.current = Some(
                configured_reader(
                    path,
                    self.batch_size,
                    self.columns.as_ref(),
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
    metrics: ScanMetrics,
) -> ParquetReader {
    let reader = ParquetReader::new(path)
        .with_batch_size(batch_size)
        .with_metrics(metrics);
    match columns {
        Some(columns) => reader.with_columns(columns.clone()),
        None => reader,
    }
}

fn active_files(table_path: &Path) -> Result<Vec<PathBuf>> {
    let log_path = table_path.join("_delta_log");
    let mut logs: Vec<PathBuf> = fs::read_dir(&log_path)
        .map_err(|error| delta_error(format!("cannot read '{}': {error}", log_path.display())))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.len() == DELTA_VERSION_WIDTH
                            && name.chars().all(|character| character.is_ascii_digit())
                    })
        })
        .collect();
    logs.sort();
    validate_log_sequence(&logs)?;

    let mut active = BTreeSet::new();
    for log in logs {
        let content = fs::read_to_string(&log)?;
        for (line_number, line) in content.lines().enumerate() {
            let action: Value = serde_json::from_str(line).map_err(|error| {
                delta_error(format!(
                    "invalid Delta action in '{}' at line {}: {error}",
                    log.display(),
                    line_number + 1
                ))
            })?;
            if let Some(path) = action.pointer("/add/path").and_then(Value::as_str) {
                active.insert(validate_relative_path(path)?);
            }
            if let Some(path) = action.pointer("/remove/path").and_then(Value::as_str) {
                active.remove(&validate_relative_path(path)?);
            }
        }
    }
    Ok(active
        .into_iter()
        .map(|path| table_path.join(path))
        .collect())
}

fn validate_log_sequence(logs: &[PathBuf]) -> Result<()> {
    if logs.is_empty() {
        return Err(delta_error(
            "Delta transaction log contains no JSON commits",
        ));
    }
    for (expected, path) in logs.iter().enumerate() {
        let actual = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<usize>().ok());
        if actual != Some(expected) {
            return Err(delta_error(format!(
                "Delta JSON history is incomplete at version {expected}; checkpoint replay is not supported yet"
            )));
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(delta_error("Delta action contains an unsafe file path"));
    }
    Ok(path)
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
        assert!(error.to_string().contains("unsafe file path"));
        fs::remove_dir_all(&table).expect("test table should be removed");
    }
}
