use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use std::sync::Arc;

pub struct QueryResult {
    pub schema: Arc<Schema>,
    pub batches: Vec<RecordBatch>,
}

impl QueryResult {
    pub fn row_count(&self) -> usize {
        self.batches.iter().map(|b| b.num_rows()).sum()
    }
}
