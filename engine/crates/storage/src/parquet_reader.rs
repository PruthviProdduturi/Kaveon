use arrow::record_batch::RecordBatch;
use kaveon_core::Result;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::fs::File;
use std::path::Path;

pub struct ParquetReader {
    path: std::path::PathBuf,
    batch_size: usize,
    columns: Option<Vec<String>>,
}

impl ParquetReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            batch_size: 8192,
            columns: None,
        }
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_columns(mut self, cols: Vec<String>) -> Self {
        self.columns = Some(cols);
        self
    }

    pub fn read_batches(&self) -> Result<Vec<RecordBatch>> {
        let file = File::open(&self.path)?;
        let mut builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| kaveon_core::KaveonError::Storage(e.to_string()))?
            .with_batch_size(self.batch_size);

        if let Some(ref cols) = self.columns {
            let schema = builder.schema().clone();
            let projection: Vec<usize> = cols
                .iter()
                .filter_map(|name| schema.index_of(name).ok())
                .collect();
            builder = builder.with_projection(parquet::arrow::ProjectionMask::roots(
                builder.parquet_schema(),
                projection,
            ));
        }

        let reader = builder
            .build()
            .map_err(|e| kaveon_core::KaveonError::Storage(e.to_string()))?;

        let batches: Vec<RecordBatch> = reader
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| kaveon_core::KaveonError::Arrow(e))?;

        Ok(batches)
    }
}
