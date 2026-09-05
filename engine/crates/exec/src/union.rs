use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Result};

pub struct UnionOperator {
    inputs: Vec<Box<dyn BatchOperator>>,
    current: usize,
    schema: SchemaRef,
}

impl UnionOperator {
    pub fn new(inputs: Vec<Box<dyn BatchOperator>>) -> Self {
        let schema = inputs[0].schema().clone();
        Self {
            inputs,
            current: 0,
            schema,
        }
    }
}

impl BatchOperator for UnionOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        while self.current < self.inputs.len() {
            if let Some(batch) = self.inputs[self.current].next_batch()? {
                return Ok(Some(batch));
            }
            self.current += 1;
        }
        Ok(None)
    }
}
