use arrow::compute;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, Result};

use crate::expr_eval::evaluate_predicate;

pub struct FilterOperator {
    source: Box<dyn BatchOperator>,
    predicate: Expr,
    schema: SchemaRef,
    memory: Option<kaveon_core::OperatorMemoryAccount>,
}

impl FilterOperator {
    pub fn new(source: Box<dyn BatchOperator>, predicate: Expr) -> Self {
        let schema = source.schema().clone();
        Self {
            source,
            predicate,
            schema,
            memory: None,
        }
    }

    pub fn with_memory(mut self, memory: kaveon_core::OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }
}

impl BatchOperator for FilterOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        while let Some(batch) = self.source.next_batch()? {
            let _workspace = self
                .memory
                .as_ref()
                .map(|memory| {
                    memory.reserve(
                        (batch.get_array_memory_size() as u64)
                            .saturating_mul(3)
                            .saturating_add((batch.num_rows() as u64).saturating_mul(16)),
                    )
                })
                .transpose()?;
            let mask = crate::expr_eval::with_expression_memory(self.memory.as_ref(), || {
                evaluate_predicate(&self.predicate, &batch)
            })?;
            let filtered = compute::filter_record_batch(&batch, &mask)?;
            if filtered.num_rows() > 0 {
                return Ok(Some(filtered));
            }
        }
        Ok(None)
    }
}
