use std::collections::VecDeque;

use arrow::array::{ArrayRef, UInt32Builder};
use arrow::compute::take;
use arrow::record_batch::RecordBatch;
use arrow::row::{RowConverter, SortField};
use kaveon_core::{KaveonError, Result};

const FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

pub struct HashPartitioner {
    key_indices: Vec<usize>,
    partition_count: usize,
}

impl HashPartitioner {
    pub fn try_new(
        schema: &arrow::datatypes::SchemaRef,
        columns: &[String],
        partition_count: usize,
    ) -> Result<Self> {
        if columns.is_empty() {
            return Err(KaveonError::Execution(
                "hash partitioning requires at least one key column".into(),
            ));
        }
        if partition_count == 0 {
            return Err(KaveonError::Execution(
                "hash partition count must be greater than zero".into(),
            ));
        }
        let key_indices = columns
            .iter()
            .map(|column| {
                schema.index_of(column).map_err(|_| {
                    KaveonError::Execution(format!(
                        "hash partition key '{column}' is not in the input schema"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            key_indices,
            partition_count,
        })
    }

    pub fn partition(&self, batch: &RecordBatch) -> Result<Vec<RecordBatch>> {
        let key_columns = self
            .key_indices
            .iter()
            .map(|index| batch.column(*index).clone())
            .collect::<Vec<ArrayRef>>();
        let fields = key_columns
            .iter()
            .map(|column| SortField::new(column.data_type().clone()))
            .collect();
        let converter = RowConverter::new(fields)?;
        let rows = converter.convert_columns(&key_columns)?;
        let mut indices = (0..self.partition_count)
            .map(|_| UInt32Builder::new())
            .collect::<Vec<_>>();
        for row_index in 0..batch.num_rows() {
            let hash = stable_hash(rows.row(row_index).as_ref());
            let partition = (hash % self.partition_count as u64) as usize;
            indices[partition].append_value(u32::try_from(row_index).map_err(|_| {
                KaveonError::Execution("record batch exceeds Arrow UInt32 row capacity".into())
            })?);
        }
        indices
            .into_iter()
            .map(|mut indices| {
                let indices = indices.finish();
                let columns = batch
                    .columns()
                    .iter()
                    .map(|column| take(column.as_ref(), &indices, None))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(RecordBatch::try_new(batch.schema(), columns)?)
            })
            .collect()
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

pub struct BoundedExchangeBuffer {
    batches: VecDeque<RecordBatch>,
    capacity_bytes: usize,
    buffered_bytes: usize,
}

impl BoundedExchangeBuffer {
    pub fn new(capacity_bytes: usize) -> Result<Self> {
        if capacity_bytes == 0 {
            return Err(KaveonError::Execution(
                "exchange buffer capacity must be greater than zero".into(),
            ));
        }
        Ok(Self {
            batches: VecDeque::new(),
            capacity_bytes,
            buffered_bytes: 0,
        })
    }

    pub fn try_push(&mut self, batch: RecordBatch) -> Result<()> {
        let batch_bytes = batch.get_array_memory_size();
        if batch_bytes > self.capacity_bytes.saturating_sub(self.buffered_bytes) {
            return Err(KaveonError::Execution(format!(
                "exchange buffer backpressure: {batch_bytes} bytes cannot fit in {} available bytes",
                self.capacity_bytes.saturating_sub(self.buffered_bytes)
            )));
        }
        self.buffered_bytes = self.buffered_bytes.saturating_add(batch_bytes);
        self.batches.push_back(batch);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<RecordBatch> {
        let batch = self.batches.pop_front()?;
        self.buffered_bytes = self
            .buffered_bytes
            .saturating_sub(batch.get_array_memory_size());
        Some(batch)
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedExchangeBuffer, HashPartitioner};
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("value", DataType::Int64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![
                    Some("east"),
                    Some("west"),
                    None,
                    Some("east"),
                    None,
                ])),
                Arc::new(Int64Array::from(vec![10, 20, 30, 40, 50])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn hash_partitioning_is_deterministic_and_lossless() {
        let input = batch();
        let partitioner = HashPartitioner::try_new(&input.schema(), &["key".into()], 3).unwrap();
        let first = partitioner.partition(&input).unwrap();
        let second = partitioner.partition(&input).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.iter().map(RecordBatch::num_rows).sum::<usize>(), 5);

        let east_partitions = first
            .iter()
            .enumerate()
            .filter_map(|(partition, batch)| {
                let keys = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                keys.iter()
                    .any(|key| key == Some("east"))
                    .then_some(partition)
            })
            .collect::<Vec<_>>();
        assert_eq!(east_partitions.len(), 1);
        let east_partition = &first[east_partitions[0]];
        let keys = east_partition
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(keys.iter().filter(|key| *key == Some("east")).count(), 2);
        let null_partitions = first
            .iter()
            .enumerate()
            .filter_map(|(partition, batch)| {
                let keys = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                keys.iter().any(|key| key.is_none()).then_some(partition)
            })
            .collect::<Vec<_>>();
        assert_eq!(null_partitions.len(), 1);
        let null_keys = first[null_partitions[0]]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(null_keys.iter().filter(Option::is_none).count(), 2);
    }

    #[test]
    fn bounded_buffer_applies_and_releases_backpressure() {
        let input = batch();
        let batch_bytes = input.get_array_memory_size();
        let mut buffer = BoundedExchangeBuffer::new(batch_bytes).unwrap();
        buffer.try_push(input.clone()).unwrap();
        assert!(buffer.try_push(input.clone()).is_err());
        assert_eq!(buffer.buffered_bytes(), batch_bytes);
        assert_eq!(buffer.pop(), Some(input));
        assert!(buffer.is_empty());
        assert_eq!(buffer.buffered_bytes(), 0);
    }

    #[test]
    fn rejects_invalid_exchange_configuration() {
        let input = batch();
        assert!(HashPartitioner::try_new(&input.schema(), &[], 2).is_err());
        assert!(HashPartitioner::try_new(&input.schema(), &["missing".into()], 2).is_err());
        assert!(HashPartitioner::try_new(&input.schema(), &["key".into()], 0).is_err());
        assert!(BoundedExchangeBuffer::new(0).is_err());
    }
}
