use crate::Result;
use arrow::record_batch::RecordBatch;

pub trait BatchSource {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;
}

pub trait BatchOperator {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;
}

pub fn collect_batches(source: &mut dyn BatchOperator) -> Result<Vec<RecordBatch>> {
    let mut out = Vec::new();
    while let Some(batch) = source.next_batch()? {
        out.push(batch);
    }
    Ok(out)
}
