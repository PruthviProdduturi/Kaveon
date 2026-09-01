use arrow::record_batch::RecordBatch;
use kaveon_core::Result;
use kaveon_storage::parquet_reader::ParquetReader;
use std::path::Path;

pub struct ScanOperator {
    reader: ParquetReader,
}

impl ScanOperator {
    pub fn new(path: impl AsRef<Path>, columns: Option<Vec<String>>) -> Self {
        let mut reader = ParquetReader::new(path);
        if let Some(cols) = columns {
            reader = reader.with_columns(cols);
        }
        Self { reader }
    }

    pub fn execute(&self) -> Result<Vec<RecordBatch>> {
        self.reader.read_batches()
    }
}
