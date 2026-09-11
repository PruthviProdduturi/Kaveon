use crate::{
    ObjectBatchSource, ObjectLocation, ObjectParquetReader, ParquetFileMetadata, ScanMetrics,
    ScanPartition,
    delta_snapshot::{DeltaSnapshot, blocking, resolve_snapshot},
    object_reader::error,
};
use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use futures::{StreamExt, stream};
use kaveon_core::{BatchSource, Result, StoragePredicate};
use object_store::{ObjectStore, path::Path};
use std::sync::Arc;

const METADATA_READ_CONCURRENCY: usize = 16;

pub struct ObjectDeltaReader {
    location: ObjectLocation,
    version: Option<u64>,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    batch_size: usize,
}
impl ObjectDeltaReader {
    pub fn new(store: Arc<dyn ObjectStore>, path: Path) -> Self {
        Self {
            location: ObjectLocation { store, path },
            version: None,
            columns: None,
            predicate: None,
            partition: None,
            batch_size: 8_192,
        }
    }
    pub fn from_uri(uri: &str) -> Result<Self> {
        let location = ObjectLocation::from_uri(uri.trim_end_matches('/'))?;
        Ok(Self::new(location.store, location.path))
    }
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
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
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }
    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(predicate);
        self
    }
    pub fn snapshot(&self) -> Result<DeltaSnapshot> {
        let location = self.location.clone();
        let version = self.version;
        blocking(async move { resolve_snapshot(location.store, &location.path, version).await })
    }

    /// Returns whether `version` is still the latest committed Delta version.
    ///
    /// Delta commit files are immutable and their numeric versions are
    /// consecutive. A successful, strongly consistent `HEAD` for version
    /// `N + 1` therefore invalidates a cached version `N`; `NotFound`
    /// establishes that `N` is current at the time of the request unless log
    /// cleanup has replaced newer JSON commits with a checkpoint. Reading
    /// `_last_checkpoint` after the probe covers that valid cleanup case.
    pub(crate) fn is_latest_version(&self, version: u64) -> Result<bool> {
        let Some(next) = version.checked_add(1) else {
            return Ok(true);
        };
        let location = self.location.clone();
        blocking(async move {
            let path = location
                .path
                .child("_delta_log")
                .child(format!("{next:020}.json"));
            match location.store.head(&path).await {
                Ok(_) => return Ok(false),
                Err(object_store::Error::NotFound { .. }) => {}
                Err(source) => return Err(error(source.to_string())),
            }
            let checkpoint = location.path.child("_delta_log").child("_last_checkpoint");
            let result = match location.store.get(&checkpoint).await {
                Ok(result) => result,
                Err(object_store::Error::NotFound { .. }) => return Ok(true),
                Err(source) => return Err(error(source.to_string())),
            };
            if result.meta.size > 64 * 1024 {
                return Err(error("Delta _last_checkpoint exceeds 64 KiB"));
            }
            let bytes = result
                .bytes()
                .await
                .map_err(|source| error(source.to_string()))?;
            let checkpoint_version = serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(|source| error(source.to_string()))?
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| error("Delta _last_checkpoint has no valid version"))?;
            Ok(checkpoint_version <= version)
        })
    }
    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        let snapshot = self.snapshot()?;
        self.metadata_for_snapshot(snapshot)
    }

    /// Reads exact file statistics for an already resolved immutable snapshot.
    /// Reusing the snapshot prevents a second transaction-log traversal and
    /// bounded concurrency avoids one network round trip per active file in
    /// series without allowing large tables to create unbounded requests.
    pub(crate) fn metadata_for_snapshot(
        &self,
        snapshot: DeltaSnapshot,
    ) -> Result<ParquetFileMetadata> {
        if snapshot.files.is_empty() {
            return Ok(ParquetFileMetadata {
                schema: snapshot
                    .schema
                    .ok_or_else(|| error("empty Delta snapshot has no logical schema"))?,
                row_count: 0,
                row_group_count: 0,
            });
        }
        let store = self.location.store.clone();
        blocking(async move {
            let mut metadata = stream::iter(snapshot.files.into_iter().map(|path| {
                let store = store.clone();
                async move { ObjectParquetReader::new(store, path).metadata().await }
            }))
            .buffered(METADATA_READ_CONCURRENCY);
            let mut combined = metadata
                .next()
                .await
                .ok_or_else(|| error("Delta snapshot has no active files"))??;
            while let Some(next) = metadata.next().await {
                let next = next?;
                if next.schema != combined.schema {
                    return Err(error("Delta snapshot has incompatible physical schemas"));
                }
                combined.row_count = combined
                    .row_count
                    .checked_add(next.row_count)
                    .ok_or_else(|| error("Delta row count overflow"))?;
                combined.row_group_count = combined
                    .row_group_count
                    .checked_add(next.row_group_count)
                    .ok_or_else(|| error("Delta row-group count overflow"))?;
            }
            if let Some(schema) = snapshot.schema {
                crate::delta_snapshot::validate_physical_schema(&schema, &combined.schema)?;
                combined.schema = schema;
            }
            Ok(combined)
        })
    }
    pub fn read_blocking(self) -> Result<ObjectDeltaSource> {
        if self.batch_size == 0 {
            return Err(error("batch size must be greater than zero"));
        }
        let started = std::time::Instant::now();
        let snapshot = self.snapshot()?;
        if snapshot.files.is_empty() {
            let schema = snapshot
                .schema
                .ok_or_else(|| error("empty Delta snapshot has no logical schema"))?;
            let (schema, _) =
                crate::parquet_reader::ordered_projection(schema, self.columns.as_deref())?;
            return Ok(ObjectDeltaSource {
                store: self.location.store,
                files: vec![].into_iter(),
                schema,
                current: None,
                columns: self.columns,
                predicate: self.predicate,
                batch_size: self.batch_size,
                metrics: ScanMetrics::default(),
            });
        }
        let first = snapshot
            .files
            .first()
            .cloned()
            .ok_or_else(|| error("Delta snapshot has no active files"))?;
        let metrics = ScanMetrics::default();
        metrics.snapshot_time(started.elapsed());
        let mut schema_reader =
            ObjectParquetReader::new(self.location.store.clone(), first.clone())
                .with_batch_size(self.batch_size);
        if let Some(columns) = &self.columns {
            schema_reader = schema_reader.with_columns(columns.clone());
        }
        if let Some(predicate) = &self.predicate {
            schema_reader = schema_reader.with_predicate(predicate.clone());
        }
        let source = schema_reader.read_blocking()?;
        let mut schema = source.schema().clone();
        drop(source);
        if let Some(logical_schema) = snapshot.schema {
            let store = self.location.store.clone();
            let physical_schema = blocking(async move {
                Ok(ObjectParquetReader::new(store, first)
                    .metadata()
                    .await?
                    .schema)
            })?;
            crate::delta_snapshot::validate_physical_schema(&logical_schema, &physical_schema)?;
            schema =
                crate::parquet_reader::ordered_projection(logical_schema, self.columns.as_deref())?
                    .0;
        }
        let files = snapshot
            .files
            .into_iter()
            .enumerate()
            .filter_map(|(i, path)| {
                self.partition
                    .is_none_or(|partition| partition.contains(i))
                    .then_some(path)
            })
            .collect::<Vec<_>>()
            .into_iter();
        Ok(ObjectDeltaSource {
            store: self.location.store,
            files,
            schema,
            current: None,
            columns: self.columns,
            predicate: self.predicate,
            batch_size: self.batch_size,
            metrics,
        })
    }
}
pub struct ObjectDeltaSource {
    store: Arc<dyn ObjectStore>,
    files: std::vec::IntoIter<Path>,
    schema: SchemaRef,
    current: Option<ObjectBatchSource>,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    batch_size: usize,
    metrics: ScanMetrics,
}
impl ObjectDeltaSource {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}
impl BatchSource for ObjectDeltaSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(current) = &mut self.current {
                if let Some(batch) = current.next_batch()? {
                    return Ok(Some(RecordBatch::try_new(
                        self.schema.clone(),
                        batch.columns().to_vec(),
                    )?));
                }
                self.current = None;
            }
            let Some(path) = self.files.next() else {
                return Ok(None);
            };
            let mut reader = ObjectParquetReader::new(self.store.clone(), path)
                .with_batch_size(self.batch_size)
                .with_metrics(self.metrics.clone());
            if let Some(columns) = &self.columns {
                reader = reader.with_columns(columns.clone());
            }
            if let Some(predicate) = &self.predicate {
                reader = reader.with_predicate(predicate.clone());
            }
            let source = reader.read_blocking()?;
            crate::delta_snapshot::validate_physical_schema(&self.schema, source.schema())?;
            self.current = Some(source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::Int64Array,
        datatypes::{DataType, Field, Schema},
    };
    use object_store::memory::InMemory;

    #[test]
    fn latest_version_probe_invalidates_on_the_next_atomic_commit() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(store.put(
                &Path::from("table/_delta_log/00000000000000000000.json"),
                b"{}".to_vec().into(),
            ))
            .unwrap();
        let reader = ObjectDeltaReader::new(store.clone(), Path::from("table"));
        assert!(reader.is_latest_version(0).unwrap());

        runtime
            .block_on(store.put(
                &Path::from("table/_delta_log/00000000000000000001.json"),
                b"{}".to_vec().into(),
            ))
            .unwrap();
        assert!(!reader.is_latest_version(0).unwrap());
        assert!(reader.is_latest_version(1).unwrap());

        runtime
            .block_on(store.delete(&Path::from("table/_delta_log/00000000000000000001.json")))
            .unwrap();
        runtime
            .block_on(store.put(
                &Path::from("table/_delta_log/_last_checkpoint"),
                br#"{"version":2,"size":1}"#.to_vec().into(),
            ))
            .unwrap();
        assert!(!reader.is_latest_version(0).unwrap());
    }

    #[test]
    fn empty_object_snapshot_has_schema_without_opening_data_files() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let schema = serde_json::json!({"type":"struct","fields":[{"name":"id","type":"long","nullable":true}]}).to_string();
        let action = serde_json::json!({"metaData":{"schemaString":schema}}).to_string();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(store.put(
                &Path::from("table/_delta_log/00000000000000000000.json"),
                action.into_bytes().into(),
            ))
            .unwrap();
        let reader = ObjectDeltaReader::new(store, Path::from("table"));
        assert_eq!(reader.metadata().unwrap().row_count, 0);
        let mut source = reader.read_blocking().unwrap();
        assert_eq!(source.schema().field(0).data_type(), &DataType::Int64);
        assert!(source.next_batch().unwrap().is_none());
    }

    #[test]
    fn pinned_version_survives_new_commit_and_file_partitions_cover_once() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (name, values) in [
            ("one", vec![1, 2]),
            ("two", vec![3, 4]),
            ("three", vec![5, 6, 7]),
        ] {
            let batch =
                RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])
                    .unwrap();
            let mut writer =
                parquet::arrow::ArrowWriter::try_new(Vec::new(), schema.clone(), None).unwrap();
            writer.write(&batch).unwrap();
            runtime
                .block_on(store.put(
                    &Path::from(format!("table/{name}.parquet")),
                    writer.into_inner().unwrap().into(),
                ))
                .unwrap();
        }
        runtime
            .block_on(
                store.put(
                    &Path::from("table/_delta_log/00000000000000000000.json"),
                    b"{\"add\":{\"path\":\"one.parquet\"}}\n{\"add\":{\"path\":\"two.parquet\"}}"
                        .to_vec()
                        .into(),
                ),
            )
            .unwrap();
        let pinned_reader = ObjectDeltaReader::new(store.clone(), Path::from("table"));
        let pinned_snapshot = pinned_reader.snapshot().unwrap();
        let version = pinned_snapshot.version;
        runtime.block_on(store.put(&Path::from("table/_delta_log/00000000000000000001.json"), b"{\"remove\":{\"path\":\"one.parquet\"}}\n{\"add\":{\"path\":\"three.parquet\"}}".to_vec().into())).unwrap();
        assert_eq!(
            pinned_reader
                .metadata_for_snapshot(pinned_snapshot)
                .unwrap()
                .row_count,
            4
        );
        assert_eq!(
            ObjectDeltaReader::new(store.clone(), Path::from("table"))
                .metadata()
                .unwrap()
                .row_count,
            5
        );
        let mut values = Vec::new();
        for partition in 0..3 {
            let mut source = ObjectDeltaReader::new(store.clone(), Path::from("table"))
                .with_version(version)
                .with_columns(vec!["x".into()])
                .with_batch_size(1)
                .with_partition(ScanPartition::new(partition, 3).unwrap())
                .read_blocking()
                .unwrap();
            assert_eq!(source.schema(), &schema);
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
            assert!(source.next_batch().unwrap().is_none());
        }
        values.sort_unstable();
        assert_eq!(values, vec![1, 2, 3, 4]);
        let mut pruned = ObjectDeltaReader::new(store.clone(), Path::from("table"))
            .with_version(version)
            .with_predicate(StoragePredicate::Compare {
                column: "x".into(),
                op: kaveon_core::CompareOp::Lt,
                value: kaveon_core::ScalarValue::Int64(3),
            })
            .read_blocking()
            .unwrap();
        let mut selected = Vec::new();
        while let Some(batch) = pruned.next_batch().unwrap() {
            selected.extend(
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
        assert_eq!(selected, vec![1, 2]);
        assert_eq!(pruned.metrics().snapshot().row_groups_pruned(), 1);
        assert_eq!(
            ObjectDeltaReader::new(store, Path::from("table"))
                .snapshot()
                .unwrap()
                .version,
            1
        );
    }
}
