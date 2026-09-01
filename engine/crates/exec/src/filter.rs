use arrow::compute;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, Result};

use crate::expr_eval::evaluate_predicate;

pub struct FilterOperator {
    source: Box<dyn BatchOperator>,
    predicate: Expr,
    schema: SchemaRef,
}

impl FilterOperator {
    pub fn new(source: Box<dyn BatchOperator>, predicate: Expr) -> Self {
        let schema = source.schema().clone();
        Self {
            source,
            predicate,
            schema,
        }
    }
}

impl BatchOperator for FilterOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        while let Some(batch) = self.source.next_batch()? {
            let mask = evaluate_predicate(&self.predicate, &batch)?;
            let filtered = compute::filter_record_batch(&batch, &mask)?;
            if filtered.num_rows() > 0 {
                return Ok(Some(filtered));
            }
        }
        Ok(None)
    }
}
