use crate::{
    ObjectBatchSource, ObjectLocation, ObjectParquetReader, ParquetFileMetadata, ScanMetrics,
    ScanPartition,
    delta_snapshot::{DeltaSnapshot, blocking, resolve_snapshot},
    object_reader::error,
};
use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use kaveon_core::{BatchSource, Result, StoragePredicate};
use object_store::{ObjectStore, path::Path};
use std::sync::Arc;

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
    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        let snapshot = self.snapshot()?;
        if snapshot.files.is_empty() {
            return Ok(ParquetFileMetadata {
                schema: snapshot
                    .schema
                    .ok_or_else(|| error("empty Delta snapshot has no logical schema"))?,
                row_count: 0,
                row_group_count: 0,
            });
        }
        let first = snapshot
            .files
            .first()
            .cloned()
            .ok_or_else(|| error("Delta snapshot has no active files"))?;
        let store = self.location.store.clone();
        blocking(async move {
            let mut metadata = ObjectParquetReader::new(store.clone(), first)
                .metadata()
                .await?;
            for path in snapshot.files.into_iter().skip(1) {
                let next = ObjectParquetReader::new(store.clone(), path)
                    .metadata()
                    .await?;
                if next.schema != metadata.schema {
                    return Err(error("Delta snapshot has incompatible physical schemas"));
                }
                metadata.row_count = metadata
                    .row_count
                    .checked_add(next.row_count)
                    .ok_or_else(|| error("Delta row count overflow"))?;
                metadata.row_group_count += next.row_group_count;
            }
            if let Some(schema) = snapshot.schema {
                crate::delta_snapshot::validate_physical_schema(&schema, &metadata.schema)?;
                metadata.schema = schema;
            }
            Ok(metadata)
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
            ("three", vec![5, 6]),
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
        let version = ObjectDeltaReader::new(store.clone(), Path::from("table"))
            .snapshot()
            .unwrap()
            .version;
        runtime.block_on(store.put(&Path::from("table/_delta_log/00000000000000000001.json"), b"{\"remove\":{\"path\":\"one.parquet\"}}\n{\"add\":{\"path\":\"three.parquet\"}}".to_vec().into())).unwrap();
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
