use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Result};

pub struct LimitOperator {
    source: Box<dyn BatchOperator>,
    remaining: usize,
    schema: SchemaRef,
}

impl LimitOperator {
    pub fn new(source: Box<dyn BatchOperator>, limit: usize) -> Self {
        let schema = source.schema().clone();
        Self {
            source,
            remaining: limit,
            schema,
        }
    }
}

impl BatchOperator for LimitOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let Some(batch) = self.source.next_batch()? else {
            return Ok(None);
        };
        let rows = batch.num_rows();
        if rows <= self.remaining {
            self.remaining -= rows;
            Ok(Some(batch))
        } else {
            let sliced = batch.slice(0, self.remaining);
            self.remaining = 0;
            Ok(Some(sliced))
        }
    }
}
