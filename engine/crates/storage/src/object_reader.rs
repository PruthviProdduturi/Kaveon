//! Shared object-store range reader. Credentials come from provider chains, never URIs.
use crate::{
    ScanMetrics, ScanPartition,
    parquet_reader::{
        matching_row_groups, projection_indices, record_selection_metrics, validate_predicate,
    },
};
use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use futures::StreamExt;
use kaveon_core::{BatchSource, KaveonError, Result, StoragePredicate};
use object_store::{ObjectStore, aws::AmazonS3Builder, azure::MicrosoftAzureBuilder, path::Path};
use parquet::arrow::{
    ParquetRecordBatchStreamBuilder, ProjectionMask, async_reader::ParquetObjectReader,
};
use std::sync::{Arc, mpsc};

#[derive(Clone)]
pub struct ObjectLocation {
    pub store: Arc<dyn ObjectStore>,
    pub path: Path,
}

impl ObjectLocation {
    pub fn from_uri(uri: &str) -> Result<Self> {
        if uri.contains(['?', '#', '\\']) {
            return Err(error(
                "object URI cannot contain query, fragment, or backslash",
            ));
        }
        if let Some(rest) = uri.strip_prefix("s3://") {
            let (bucket, key) = rest
                .split_once('/')
                .ok_or_else(|| error("S3 URI requires bucket/key"))?;
            if bucket.is_empty() || bucket.contains('@') {
                return Err(error("invalid S3 bucket authority"));
            }
            let path = relative_path(key)?;
            let store = AmazonS3Builder::from_env()
                .with_bucket_name(bucket)
                .build()
                .map_err(storage_error)?;
            return Ok(Self {
                store: Arc::new(store),
                path,
            });
        }
        let rest = uri
            .strip_prefix("abfss://")
            .ok_or_else(|| error("expected s3:// or abfss:// object URI"))?;
        let (authority, key) = rest
            .split_once('/')
            .ok_or_else(|| error("ADLS URI requires an object path"))?;
        let (container, host) = authority
            .split_once('@')
            .ok_or_else(|| error("ADLS URI requires container@account authority"))?;
        let account = host
            .strip_suffix(".dfs.core.windows.net")
            .ok_or_else(|| error("ADLS URI requires dfs.core.windows.net"))?;
        if container.is_empty()
            || account.is_empty()
            || !account.bytes().all(|c| c.is_ascii_alphanumeric())
        {
            return Err(error("invalid ADLS authority"));
        }
        let path = relative_path(key)?;
        let store = MicrosoftAzureBuilder::from_env()
            .with_account(account)
            .with_container_name(container)
            .build()
            .map_err(storage_error)?;
        Ok(Self {
            store: Arc::new(store),
            path,
        })
    }
}

pub(crate) fn relative_path(value: &str) -> Result<Path> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '?', '#'])
        || value.split('/').any(|part| matches!(part, ".." | "." | ""))
    {
        return Err(error(
            "object path must be a nonempty normalized relative path",
        ));
    }
    Path::parse(value).map_err(storage_error)
}

#[derive(Clone)]
pub struct ObjectParquetReader {
    location: ObjectLocation,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    metrics: ScanMetrics,
}

pub struct ObjectBatchSource {
    schema: SchemaRef,
    receiver: mpsc::Receiver<Result<Option<RecordBatch>>>,
    metrics: ScanMetrics,
    exhausted: bool,
}

impl ObjectBatchSource {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

impl BatchSource for ObjectBatchSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.exhausted {
            return Ok(None);
        }
        let batch = self
            .receiver
            .recv()
            .map_err(|_| error("object reader terminated without an end-of-stream marker"))??;
        self.exhausted = batch.is_none();
        Ok(batch)
    }
}

impl ObjectParquetReader {
    pub fn new(store: Arc<dyn ObjectStore>, path: Path) -> Self {
        Self {
            location: ObjectLocation { store, path },
            batch_size: 8_192,
            columns: None,
            predicate: None,
            partition: None,
            metrics: ScanMetrics::default(),
        }
    }
    pub fn from_uri(uri: &str) -> Result<Self> {
        let location = ObjectLocation::from_uri(uri)?;
        Ok(Self::new(location.store, location.path))
    }
    pub fn with_batch_size(mut self, value: usize) -> Self {
        self.batch_size = value;
        self
    }
    pub fn with_columns(mut self, value: Vec<String>) -> Self {
        self.columns = Some(value);
        self
    }
    pub fn with_predicate(mut self, value: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(previous) => StoragePredicate::And(vec![previous, value]),
            None => value,
        });
        self
    }
    pub fn with_partition(mut self, value: ScanPartition) -> Self {
        self.partition = Some(value);
        self
    }
    pub fn with_metrics(mut self, value: ScanMetrics) -> Self {
        self.metrics = value;
        self
    }

    pub async fn metadata(&self) -> Result<crate::ParquetFileMetadata> {
        let meta = self
            .location
            .store
            .head(&self.location.path)
            .await
            .map_err(storage_error)?;
        let reader = ParquetObjectReader::new(self.location.store.clone(), meta);
        let builder = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .map_err(storage_error)?;
        Ok(crate::ParquetFileMetadata {
            schema: builder.schema().clone(),
            row_count: u64::try_from(builder.metadata().file_metadata().num_rows())
                .map_err(storage_error)?,
            row_group_count: builder.metadata().num_row_groups(),
        })
    }

    pub fn read_blocking(self) -> Result<ObjectBatchSource> {
        if self.batch_size == 0 {
            return Err(error("batch size must be greater than zero"));
        }
        let (initial_tx, initial_rx) = mpsc::sync_channel(1);
        let (tx, rx) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-object-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(failure) => {
                        let _ = initial_tx.send(Err(storage_error(failure)));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let built = async {
                        let started = std::time::Instant::now();
                        self.metrics.files_considered(1);
                        let meta = self
                            .location
                            .store
                            .head(&self.location.path)
                            .await
                            .map_err(storage_error)?;
                        let reader = ParquetObjectReader::new(self.location.store, meta);
                        let mut builder = ParquetRecordBatchStreamBuilder::new(reader)
                            .await
                            .map_err(storage_error)?
                            .with_batch_size(self.batch_size);
                        self.metrics.footer_time(started.elapsed());
                        self.metrics.file_opened();
                        let schema = builder.schema().clone();
                        let projection = self
                            .columns
                            .as_ref()
                            .map(|columns| projection_indices(&schema, columns))
                            .transpose()?;
                        if let Some(indices) = &projection {
                            let mask =
                                ProjectionMask::roots(builder.parquet_schema(), indices.clone());
                            builder = builder.with_projection(mask);
                        }
                        let considered = builder.metadata().num_row_groups();
                        let mut groups = if let Some(predicate) = &self.predicate {
                            validate_predicate(predicate, &schema)?;
                            matching_row_groups(builder.metadata().as_ref(), &schema, predicate)
                        } else {
                            (0..considered).collect()
                        };
                        if let Some(partition) = self.partition {
                            groups.retain(|&ordinal| partition.contains(ordinal));
                        }
                        record_selection_metrics(
                            builder.metadata().as_ref(),
                            &groups,
                            projection.as_deref(),
                            &self.metrics,
                        );
                        builder
                            .with_row_groups(groups)
                            .build()
                            .map_err(storage_error)
                    }
                    .await;
                    let mut stream = match built {
                        Ok(stream) => stream,
                        Err(failure) => {
                            let _ = initial_tx.send(Err(failure));
                            return;
                        }
                    };
                    let (schema, output_projection) =
                        match crate::parquet_reader::ordered_projection(
                            stream.schema().clone(),
                            self.columns.as_deref(),
                        ) {
                            Ok(projection) => projection,
                            Err(failure) => {
                                let _ = initial_tx.send(Err(failure));
                                return;
                            }
                        };
                    if initial_tx.send(Ok((schema, self.metrics.clone()))).is_err() {
                        return;
                    }
                    loop {
                        let start = std::time::Instant::now();
                        let next = stream.next().await;
                        self.metrics.read_time(start.elapsed());
                        match next {
                            Some(Ok(batch)) => {
                                let batch = match &output_projection {
                                    Some(indices) => match batch.project(indices) {
                                        Ok(batch) => batch,
                                        Err(failure) => {
                                            let _ = tx.send(Err(storage_error(failure)));
                                            break;
                                        }
                                    },
                                    None => batch,
                                };
                                self.metrics.emitted(batch.num_rows());
                                if tx.send(Ok(Some(batch))).is_err() {
                                    break;
                                }
                            }
                            Some(Err(failure)) => {
                                let _ = tx.send(Err(storage_error(failure)));
                                break;
                            }
                            None => {
                                let _ = tx.send(Ok(None));
                                break;
                            }
                        }
                    }
                });
            })
            .map_err(storage_error)?;
        let (schema, metrics) = initial_rx
            .recv()
            .map_err(|_| error("object reader terminated before initialization"))??;
        Ok(ObjectBatchSource {
            schema,
            receiver: rx,
            metrics,
            exhausted: false,
        })
    }
}

pub(crate) fn error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}
pub(crate) fn storage_error(failure: impl std::fmt::Display) -> KaveonError {
    error(failure.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::Int64Array,
        datatypes::{DataType, Field, Schema},
    };
    use object_store::memory::InMemory;
    use parquet::arrow::ArrowWriter;
    #[test]
    fn rejects_unsafe_uris_before_credentials_or_network() {
        for uri in [
            "s3://bucket/../data",
            "s3://bucket/data?secret=x",
            "s3://user@bucket/data",
            "abfss://c@a.invalid/data",
        ] {
            assert!(ObjectLocation::from_uri(uri).is_err(), "{uri}");
        }
    }
    #[test]
    fn object_ranges_project_and_partition_without_losing_rows() {
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3, 4]))],
        )
        .unwrap();
        let mut bytes = Vec::new();
        let mut writer = ArrowWriter::try_new(&mut bytes, schema, None).unwrap();
        writer.write(&batch.slice(0, 2)).unwrap();
        writer.flush().unwrap();
        writer.write(&batch.slice(2, 2)).unwrap();
        writer.close().unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from("table/data.parquet");
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(store.put(&path, bytes.into()))
            .unwrap();
        let mut rows = 0;
        for i in 0..2 {
            let metrics = ScanMetrics::default();
            let mut source = ObjectParquetReader::new(store.clone(), path.clone())
                .with_columns(vec!["x".into()])
                .with_partition(ScanPartition::new(i, 2).unwrap())
                .with_metrics(metrics.clone())
                .read_blocking()
                .unwrap();
            while let Some(batch) = source.next_batch().unwrap() {
                rows += batch.num_rows();
            }
            assert!(source.next_batch().unwrap().is_none());
            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.row_groups_considered, 2);
            assert_eq!(snapshot.row_groups_selected, 1);
            assert_eq!(snapshot.rows_selected, 2);
            assert!(snapshot.compressed_bytes_selected > 0);
        }
        assert_eq!(rows, 4);
        assert!(
            ObjectParquetReader::new(store, path)
                .with_columns(vec!["missing".into()])
                .read_blocking()
                .is_err()
        );
    }
}
