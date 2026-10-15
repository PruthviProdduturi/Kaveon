use crate::{Result, ScalarValue};
use arrow::record_batch::RecordBatch;

pub trait BatchSource {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;

    /// Give a source an exact build-side key filter before it starts reading.
    /// Storage implementations may use this to prune files or row groups;
    /// the default keeps non-storage sources correct by declining the push.
    fn apply_runtime_filter(&mut self, _column: &str, _values: &[ScalarValue]) -> Result<bool> {
        Ok(false)
    }
}

pub trait BatchOperator {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;

    /// Forward an exact build-side filter to a not-yet-started scan when the
    /// operator owns one. Operators that cannot push it keep the existing
    /// batch-level runtime-filter path.
    fn apply_runtime_filter(&mut self, _column: &str, _values: &[ScalarValue]) -> Result<bool> {
        Ok(false)
    }
}

pub fn collect_batches(source: &mut dyn BatchOperator) -> Result<Vec<RecordBatch>> {
    let mut out = Vec::new();
    while let Some(batch) = source.next_batch()? {
        out.push(batch);
    }
    Ok(out)
}
