use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Result};

pub struct OffsetOperator {
    source: Box<dyn BatchOperator>,
    remaining: usize,
}

impl OffsetOperator {
    pub fn new(source: Box<dyn BatchOperator>, count: usize) -> Self {
        Self {
            source,
            remaining: count,
        }
    }
}

impl BatchOperator for OffsetOperator {
    fn schema(&self) -> &SchemaRef {
        self.source.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        while self.remaining > 0 {
            let Some(batch) = self.source.next_batch()? else {
                return Ok(None);
            };
            let rows = batch.num_rows();
            if rows <= self.remaining {
                self.remaining -= rows;
                continue;
            }
            let keep = rows - self.remaining;
            self.remaining = 0;
            return Ok(Some(batch.slice(rows - keep, keep)));
        }
        self.source.next_batch()
    }
}
