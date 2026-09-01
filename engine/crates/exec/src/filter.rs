use arrow::array::BooleanArray;
use arrow::compute;
use arrow::record_batch::RecordBatch;
use kaveon_core::Result;

pub fn filter_batch(batch: &RecordBatch, mask: &BooleanArray) -> Result<RecordBatch> {
    let filtered = compute::filter_record_batch(batch, mask)?;
    Ok(filtered)
}
