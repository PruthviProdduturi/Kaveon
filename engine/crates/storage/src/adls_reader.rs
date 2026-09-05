use std::{
    pin::Pin,
    sync::{Arc, mpsc},
    time::Instant,
};

use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use futures::{Stream, StreamExt};
use kaveon_core::{BatchSource, KaveonError, Result, StoragePredicate};
use object_store::{ObjectStore, azure::MicrosoftAzureBuilder, path::Path};
use parquet::arrow::{
    ParquetRecordBatchStreamBuilder, ProjectionMask,
    async_reader::{ParquetObjectReader, ParquetRecordBatchStream},
};

use crate::{
    ScanMetrics, ScanPartition,
    parquet_reader::{matching_row_groups, projection_indices, validate_predicate},
};

const DEFAULT_BATCH_SIZE: usize = 8_192;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdlsAuthMode {
    #[default]
    Environment,
    AzureCli,
}

pub struct AdlsBatchStream {
    schema: SchemaRef,
    inner: Pin<Box<dyn Stream<Item = parquet::errors::Result<RecordBatch>> + Send>>,
    metrics: ScanMetrics,
}

impl AdlsBatchStream {
    #[must_use]
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    #[must_use]
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }

    pub async fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let started = Instant::now();
        let result = self.inner.next().await.transpose().map_err(parquet_error)?;
        self.metrics.read_time(started.elapsed());
        if let Some(batch) = &result {
            self.metrics.emitted(batch.num_rows());
        }
        Ok(result)
    }
}

pub struct AdlsBatchSource {
    schema: SchemaRef,
    receiver: mpsc::Receiver<Result<RecordBatch>>,
    metrics: ScanMetrics,
}

impl AdlsBatchSource {
    #[must_use]
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

impl BatchSource for AdlsBatchSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        match self.receiver.recv() {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }
}

#[derive(Clone)]
pub struct AdlsParquetReader {
    account: String,
    container: String,
    object_path: String,
    auth_mode: AdlsAuthMode,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    metrics: Option<ScanMetrics>,
}

impl AdlsParquetReader {
    pub fn new(
        account: impl Into<String>,
        container: impl Into<String>,
        object_path: impl Into<String>,
    ) -> Self {
        Self {
            account: account.into(),
            container: container.into(),
            object_path: object_path.into(),
            auth_mode: AdlsAuthMode::Environment,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
            metrics: None,
        }
    }

    pub fn from_abfss_uri(uri: &str) -> Result<Self> {
        let remainder = uri
            .strip_prefix("abfss://")
            .ok_or_else(|| storage_error("ADLS URI must begin with abfss://"))?;
        let (authority, object_path) = remainder
            .split_once('/')
            .ok_or_else(|| storage_error("ADLS URI must include an object path"))?;
        let (container, host) = authority
            .split_once('@')
            .ok_or_else(|| storage_error("ADLS URI must use container@account authority"))?;
        let account = host
            .strip_suffix(".dfs.core.windows.net")
            .ok_or_else(|| storage_error("ADLS URI must use dfs.core.windows.net"))?;
        let reader = Self::new(account, container, object_path.trim_start_matches('/'));
        reader.validate()?;
        Ok(reader)
    }

    pub fn with_auth_mode(mut self, auth_mode: AdlsAuthMode) -> Self {
        self.auth_mode = auth_mode;
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(existing) => StoragePredicate::And(vec![existing, predicate]),
            None => predicate,
        });
        self
    }

    pub fn with_partition(mut self, partition: ScanPartition) -> Self {
        self.partition = Some(partition);
        self
    }

    pub fn with_metrics(mut self, metrics: ScanMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub async fn read(&self) -> Result<AdlsBatchStream> {
        self.validate()?;
        let metrics = self.metrics.clone().unwrap_or_default();
        metrics.files_considered(1);
        let store: Arc<dyn ObjectStore> = Arc::new(
            MicrosoftAzureBuilder::from_env()
                .with_account(&self.account)
                .with_container_name(&self.container)
                .with_use_azure_cli(self.auth_mode == AdlsAuthMode::AzureCli)
                .build()
                .map_err(object_store_error)?,
        );
        let path =
            Path::parse(&self.object_path).map_err(|error| storage_error(error.to_string()))?;
        let footer_started = Instant::now();
        let metadata = store.head(&path).await.map_err(object_store_error)?;
        let object_reader = ParquetObjectReader::new(store, metadata);
        let mut builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(parquet_error)?
            .with_batch_size(self.batch_size);
        metrics.footer_time(footer_started.elapsed());
        metrics.file_opened();

        let schema = Arc::clone(builder.schema());
        if let Some(columns) = &self.columns {
            let projection = projection_indices(&schema, columns)?;
            let mask = ProjectionMask::roots(builder.parquet_schema(), projection);
            builder = builder.with_projection(mask);
        }

        let mut row_groups = if let Some(predicate) = &self.predicate {
            validate_predicate(predicate, &schema)?;
            matching_row_groups(builder.metadata().as_ref(), &schema, predicate)
        } else {
            (0..builder.metadata().num_row_groups()).collect()
        };
        let considered = builder.metadata().num_row_groups();
        if let Some(partition) = self.partition {
            row_groups.retain(|ordinal| partition.contains(*ordinal));
        }
        metrics.row_groups(
            u64::try_from(considered)
                .map_err(|_| storage_error("Parquet row-group count exceeds u64"))?,
            u64::try_from(row_groups.len())
                .map_err(|_| storage_error("selected row-group count exceeds u64"))?,
        );
        builder = builder.with_row_groups(row_groups);
        let stream: ParquetRecordBatchStream<ParquetObjectReader> =
            builder.build().map_err(parquet_error)?;
        let schema = Arc::clone(stream.schema());
        Ok(AdlsBatchStream {
            schema,
            inner: Box::pin(stream),
            metrics,
        })
    }

    pub fn read_blocking(self) -> Result<AdlsBatchSource> {
        self.validate()?;
        let (initial_sender, initial_receiver) = mpsc::sync_channel(1);
        let (batch_sender, batch_receiver) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-adls-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = initial_sender.send(Err(storage_error(format!(
                            "failed to start ADLS runtime: {error}"
                        ))));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let mut stream = match self.read().await {
                        Ok(stream) => stream,
                        Err(error) => {
                            let _ = initial_sender.send(Err(error));
                            return;
                        }
                    };
                    let schema = Arc::clone(stream.schema());
                    let metrics = stream.metrics();
                    if initial_sender.send(Ok((schema, metrics))).is_err() {
                        return;
                    }
                    loop {
                        match stream.next_batch().await {
                            Ok(Some(batch)) => {
                                if batch_sender.send(Ok(batch)).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => return,
                            Err(error) => {
                                let _ = batch_sender.send(Err(error));
                                return;
                            }
                        }
                    }
                });
            })
            .map_err(|error| storage_error(format!("failed to spawn ADLS reader: {error}")))?;
        let (schema, metrics) = initial_receiver
            .recv()
            .map_err(|_| storage_error("ADLS reader stopped before initialization"))??;
        Ok(AdlsBatchSource {
            schema,
            receiver: batch_receiver,
            metrics,
        })
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("ADLS account", self.account.as_str()),
            ("ADLS container", self.container.as_str()),
            ("ADLS object path", self.object_path.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(storage_error(format!("{name} cannot be empty")));
            }
        }
        if self.object_path.starts_with('/') || self.object_path.contains("..") {
            return Err(storage_error(
                "ADLS object path must be container-relative and cannot contain '..'",
            ));
        }
        if self.batch_size == 0 {
            return Err(storage_error("batch size must be greater than zero"));
        }
        Ok(())
    }
}

fn storage_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

fn parquet_error(error: parquet::errors::ParquetError) -> KaveonError {
    storage_error(error.to_string())
}

fn object_store_error(error: object_store::Error) -> KaveonError {
    storage_error(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identity_path_and_batch_size_without_network_access() {
        assert!(
            AdlsParquetReader::new("", "data", "table/file.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "", "table/file.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "/absolute.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "../escape.parquet")
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "table/file.parquet")
                .with_batch_size(0)
                .validate()
                .is_err()
        );
        assert!(
            AdlsParquetReader::new("account", "data", "table/file.parquet")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn parses_canonical_abfss_uri() {
        let reader = AdlsParquetReader::from_abfss_uri(
            "abfss://lake@account.dfs.core.windows.net/gold/orders/part.parquet",
        )
        .unwrap();
        assert_eq!(reader.account, "account");
        assert_eq!(reader.container, "lake");
        assert_eq!(reader.object_path, "gold/orders/part.parquet");
        assert!(AdlsParquetReader::from_abfss_uri("https://example.com/file.parquet").is_err());
        assert!(
            AdlsParquetReader::from_abfss_uri(
                "abfss://lake@account.blob.core.windows.net/file.parquet"
            )
            .is_err()
        );
    }
}
