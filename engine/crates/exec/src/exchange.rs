use std::collections::VecDeque;
use std::time::Instant;

use arrow::array::{ArrayRef, AsArray, Float32Array, Float64Array, UInt32Builder};
use arrow::compute::{cast, take};
use arrow::record_batch::RecordBatch;
use arrow::row::{RowConverter, SortField};
use kaveon_core::{KaveonError, Result};

const FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

pub struct HashPartitioner {
    key_indices: Vec<usize>,
    partition_count: usize,
    /// Nested partitioners on the same keys (an exchange, then threads,
    /// then spill partitions) must not agree: a `hash % n` inside a
    /// `hash % m` leaves most inner partitions empty. A non-zero salt mixes
    /// the hash before the modulus; zero is the exchange's plain function.
    salt: u64,
}

/// Partition levels nested inside one exchange partition.
pub const THREAD_PARTITION_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
pub const SPILL_PARTITION_SALT: u64 = 0xD1B5_4A32_D192_ED03;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HashPartitionMetrics {
    pub hash_us: u64,
    pub copy_us: u64,
    pub copy_allocations: u64,
    pub copied_bytes: u64,
}

impl HashPartitioner {
    pub fn try_new(
        schema: &arrow::datatypes::SchemaRef,
        columns: &[String],
        partition_count: usize,
    ) -> Result<Self> {
        Self::try_new_salted(schema, columns, partition_count, 0)
    }

    /// A partitioner independent of the plain one over the same keys.
    pub fn try_new_salted(
        schema: &arrow::datatypes::SchemaRef,
        columns: &[String],
        partition_count: usize,
        salt: u64,
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
                crate::expr_eval::resolve_column_index(schema, column).map_err(|_| {
                    KaveonError::Execution(format!(
                        "hash partition key '{column}' is not in the input schema"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            key_indices,
            partition_count,
            salt,
        })
    }

    pub fn partition(&self, batch: &RecordBatch) -> Result<Vec<RecordBatch>> {
        self.partition_profiled(batch).map(|(batches, _)| batches)
    }

    pub fn partition_profiled(
        &self,
        batch: &RecordBatch,
    ) -> Result<(Vec<RecordBatch>, HashPartitionMetrics)> {
        let hash_started = Instant::now();
        let key_columns = self
            .key_indices
            .iter()
            .map(|index| -> Result<ArrayRef> {
                let column = batch.column(*index);
                let column = match column.data_type() {
                    arrow::datatypes::DataType::Dictionary(_, value_type) => {
                        cast(column.as_ref(), value_type.as_ref())?
                    }
                    _ => column.clone(),
                };
                // SQL hash equality treats signed zero and NaN payloads as the
                // same key. Canonicalize before row encoding so partitions agree
                // with hash aggregate/join key equality on every worker.
                let normalized = match column.data_type() {
                    // Parquet row groups may assign different dictionary
                    // indexes to the same text value.  Hash the decoded value
                    // rather than the local dictionary code so every exchange
                    // sends an equal SQL key to the same worker.
                    arrow::datatypes::DataType::Float64 => {
                        std::sync::Arc::new(Float64Array::from_iter(
                            column
                                .as_primitive::<arrow::datatypes::Float64Type>()
                                .iter()
                                .map(|value| {
                                    value.map(|value| {
                                        if value == 0.0 {
                                            0.0
                                        } else if value.is_nan() {
                                            f64::NAN
                                        } else {
                                            value
                                        }
                                    })
                                }),
                        )) as ArrayRef
                    }
                    arrow::datatypes::DataType::Float32 => {
                        std::sync::Arc::new(Float32Array::from_iter(
                            column
                                .as_primitive::<arrow::datatypes::Float32Type>()
                                .iter()
                                .map(|value| {
                                    value.map(|value| {
                                        if value == 0.0 {
                                            0.0
                                        } else if value.is_nan() {
                                            f32::NAN
                                        } else {
                                            value
                                        }
                                    })
                                }),
                        )) as ArrayRef
                    }
                    _ => column.clone(),
                };
                Ok(normalized)
            })
            .collect::<Result<Vec<ArrayRef>>>()?;
        let nested_key = key_columns.iter().any(|column| {
            matches!(
                column.data_type(),
                arrow::datatypes::DataType::List(_)
                    | arrow::datatypes::DataType::LargeList(_)
                    | arrow::datatypes::DataType::Map(_, _)
                    | arrow::datatypes::DataType::Struct(_)
            )
        });
        let hashes = if nested_key {
            // Arrow's RowConverter intentionally does not yet support Map.
            // Keep the fast row encoding for primitive keys, but use the
            // canonical nested value key for lists/maps/rows so equal values
            // still reach the same worker.
            let mut hashes = vec![FNV_OFFSET_BASIS; batch.num_rows()];
            for column in &key_columns {
                if matches!(
                    column.data_type(),
                    arrow::datatypes::DataType::List(_)
                        | arrow::datatypes::DataType::LargeList(_)
                        | arrow::datatypes::DataType::Map(_, _)
                        | arrow::datatypes::DataType::Struct(_)
                ) {
                    for (row, hash) in hashes.iter_mut().enumerate() {
                        let key = crate::distinct::complex_value_key(column.as_ref(), row)?;
                        *hash = stable_hash_from(*hash, key.as_bytes());
                    }
                } else {
                    let converter =
                        RowConverter::new(vec![SortField::new(column.data_type().clone())])?;
                    let rows = converter.convert_columns(std::slice::from_ref(column))?;
                    for (row, hash) in hashes.iter_mut().enumerate() {
                        *hash = stable_hash_from(*hash, rows.row(row).as_ref());
                    }
                }
            }
            hashes
        } else {
            let fields = key_columns
                .iter()
                .map(|column| SortField::new(column.data_type().clone()))
                .collect();
            let converter = RowConverter::new(fields)?;
            let rows = converter.convert_columns(&key_columns)?;
            (0..batch.num_rows())
                .map(|row| stable_hash(rows.row(row).as_ref()))
                .collect()
        };
        let mut indices = (0..self.partition_count)
            .map(|_| UInt32Builder::new())
            .collect::<Vec<_>>();
        for (row_index, hash_value) in hashes.iter().enumerate() {
            let mut hash = *hash_value;
            if self.salt != 0 {
                hash = mix(hash ^ self.salt);
            }
            let partition = (hash % self.partition_count as u64) as usize;
            indices[partition].append_value(u32::try_from(row_index).map_err(|_| {
                KaveonError::Execution("record batch exceeds Arrow UInt32 row capacity".into())
            })?);
        }
        let hash_us = elapsed_us(hash_started);
        let copy_started = Instant::now();
        let mut copy_allocations = 0_u64;
        let batches = indices
            .into_iter()
            .map(|mut indices| {
                let indices = indices.finish();
                let columns = batch
                    .columns()
                    .iter()
                    .map(|column| {
                        copy_allocations = copy_allocations.saturating_add(1);
                        take(column.as_ref(), &indices, None)
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(RecordBatch::try_new(batch.schema(), columns)?)
            })
            .collect::<Result<Vec<_>>>()?;
        let copied_bytes = batches.iter().fold(0_u64, |total, batch| {
            total.saturating_add(batch.get_array_memory_size() as u64)
        });
        Ok((
            batches,
            HashPartitionMetrics {
                hash_us,
                copy_us: elapsed_us(copy_started),
                copy_allocations,
                copied_bytes,
            },
        ))
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// SplitMix64 finaliser: every input bit reaches every output bit.
pub(crate) fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    stable_hash_from(FNV_OFFSET_BASIS, bytes)
}

/// `stable_hash` of `prefix` followed by `bytes`, without joining them.
pub(crate) fn stable_hash_with_prefix(prefix: u8, bytes: &[u8]) -> u64 {
    stable_hash_from(stable_hash(&[prefix]), bytes)
}

fn stable_hash_from(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
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
    use super::{
        BoundedExchangeBuffer, HashPartitioner, SPILL_PARTITION_SALT, THREAD_PARTITION_SALT,
    };
    use arrow::array::{
        DictionaryArray, Int32Array, Int32Builder, Int64Array, ListBuilder, MapArray, StringArray,
        StructArray,
    };
    use arrow::buffer::{OffsetBuffer, ScalarBuffer};
    use arrow::datatypes::{DataType, Field, Int32Type, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    #[test]
    fn a_qualified_key_reaches_a_bare_column_and_partitions_like_the_bare_key() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("customer_id", DataType::Int64, false),
            Field::new("amount", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6, 7, 8])),
                Arc::new(Int64Array::from(vec![10, 20, 30, 40, 50, 60, 70, 80])),
            ],
        )
        .unwrap();
        let bare = HashPartitioner::try_new(&schema, &["customer_id".into()], 3).unwrap();
        let qualified = HashPartitioner::try_new(&schema, &["o.customer_id".into()], 3).unwrap();
        let rows = |partitions: Vec<RecordBatch>| {
            partitions
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(qualified.partition(&batch).unwrap()),
            rows(bare.partition(&batch).unwrap())
        );
        assert!(HashPartitioner::try_new(&schema, &["o.missing".into()], 3).is_err());
    }

    #[test]
    fn sql_equal_floats_share_hash_partitions() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::Float64,
            false,
        )]));
        let partitioner = HashPartitioner::try_new(&schema, &["key".into()], 17).unwrap();
        let partition = |value| {
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(arrow::array::Float64Array::from(vec![value]))],
            )
            .unwrap();
            partitioner
                .partition(&batch)
                .unwrap()
                .iter()
                .position(|batch| batch.num_rows() == 1)
                .unwrap()
        };
        assert_eq!(partition(-0.0), partition(0.0));
        assert_eq!(
            partition(f64::NAN),
            partition(f64::from_bits(0x7ff8_0000_0000_1234))
        );
    }

    #[test]
    fn nested_array_keys_can_be_partitioned() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
            true,
        )]));
        let mut builder = ListBuilder::new(Int32Builder::new());
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true);
        builder.values().append_value(3);
        builder.append(true);
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(builder.finish())]).unwrap();
        let partitioner = HashPartitioner::try_new(&schema, &["key".into()], 3).unwrap();
        let partitions = partitioner.partition(&batch).unwrap();
        assert_eq!(
            partitions.iter().map(|part| part.num_rows()).sum::<usize>(),
            2
        );
    }

    #[test]
    fn nested_map_and_row_keys_can_be_partitioned() {
        let map_fields: arrow::datatypes::Fields = vec![
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(Field::new("value", DataType::Int32, true)),
        ]
        .into();
        let map_entries = StructArray::new(
            map_fields.clone(),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    Some("b"),
                    Some("a"),
                ])) as Arc<_>,
                Arc::new(Int32Array::from(vec![Some(1), Some(2), Some(2), Some(1)])) as Arc<_>,
            ],
            None,
        );
        let map_schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(map_fields.clone()),
                    false,
                )),
                false,
            ),
            true,
        )]));
        let map = MapArray::try_new(
            Arc::new(Field::new("entries", DataType::Struct(map_fields), false)),
            OffsetBuffer::new(ScalarBuffer::from(vec![0_i32, 2_i32, 4_i32])),
            map_entries,
            None,
            false,
        )
        .unwrap();
        let map_batch = RecordBatch::try_new(map_schema.clone(), vec![Arc::new(map)]).unwrap();
        let map_partitions = HashPartitioner::try_new(&map_schema, &["key".into()], 3)
            .unwrap()
            .partition(&map_batch)
            .unwrap();
        assert_eq!(
            map_partitions
                .iter()
                .map(|part| part.num_rows())
                .sum::<usize>(),
            2
        );
        assert_eq!(
            map_partitions
                .iter()
                .filter(|partition| partition.num_rows() > 0)
                .count(),
            1
        );

        let row_fields: arrow::datatypes::Fields = vec![
            Arc::new(Field::new("id", DataType::Int32, false)),
            Arc::new(Field::new("name", DataType::Utf8, false)),
        ]
        .into();
        let row_schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::Struct(row_fields.clone()),
            true,
        )]));
        let row = StructArray::new(
            row_fields,
            vec![
                Arc::new(Int32Array::from(vec![1])) as Arc<_>,
                Arc::new(StringArray::from(vec![Some("one")])) as Arc<_>,
            ],
            None,
        );
        let row_batch = RecordBatch::try_new(row_schema.clone(), vec![Arc::new(row)]).unwrap();
        let row_partitions = HashPartitioner::try_new(&row_schema, &["key".into()], 3)
            .unwrap()
            .partition(&row_batch)
            .unwrap();
        assert_eq!(
            row_partitions
                .iter()
                .map(|part| part.num_rows())
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn salted_partitioners_are_independent_of_the_plain_one() {
        // One exchange partition (plain hash % 4) split again by the thread
        // and the spill partitioners: with the plain function only every
        // fourth inner partition would be non-empty; salted, all are.
        let schema = Arc::new(Schema::new(vec![Field::new("key", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(arrow::array::Int64Array::from_iter_values(
                0..20_000,
            ))],
        )
        .unwrap();
        let outer = HashPartitioner::try_new(&schema, &["key".into()], 4).unwrap();
        let one = outer.partition(&batch).unwrap().remove(0);
        assert!(one.num_rows() > 4_000);
        let plain = HashPartitioner::try_new(&schema, &["key".into()], 16).unwrap();
        let empty_plain = plain
            .partition(&one)
            .unwrap()
            .iter()
            .filter(|part| part.num_rows() == 0)
            .count();
        assert_eq!(empty_plain, 12);
        for salt in [THREAD_PARTITION_SALT, SPILL_PARTITION_SALT] {
            let salted =
                HashPartitioner::try_new_salted(&schema, &["key".into()], 16, salt).unwrap();
            let parts = salted.partition(&one).unwrap();
            assert!(parts.iter().all(|part| part.num_rows() > 100));
            assert_eq!(
                parts.iter().map(|part| part.num_rows()).sum::<usize>(),
                one.num_rows()
            );
        }
        // Thread and spill salts disagree with each other too.
        let thread =
            HashPartitioner::try_new_salted(&schema, &["key".into()], 4, THREAD_PARTITION_SALT)
                .unwrap()
                .partition(&one)
                .unwrap()
                .remove(0);
        let spill =
            HashPartitioner::try_new_salted(&schema, &["key".into()], 16, SPILL_PARTITION_SALT)
                .unwrap();
        assert!(
            spill
                .partition(&thread)
                .unwrap()
                .iter()
                .all(|part| part.num_rows() > 20)
        );
    }

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
        let (first, metrics) = partitioner.partition_profiled(&input).unwrap();
        let second = partitioner.partition(&input).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.iter().map(RecordBatch::num_rows).sum::<usize>(), 5);
        assert_eq!(metrics.copy_allocations, 6);
        assert!(metrics.copied_bytes > 0);

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
    fn dictionary_values_with_different_local_codes_share_a_partition() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )]));
        let first = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(
                DictionaryArray::<Int32Type>::try_new(
                    Int32Array::from(vec![0, 1]),
                    Arc::new(StringArray::from(vec!["east", "west"])),
                )
                .unwrap(),
            )],
        )
        .unwrap();
        let second = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(
                DictionaryArray::<Int32Type>::try_new(
                    Int32Array::from(vec![0, 1]),
                    Arc::new(StringArray::from(vec!["west", "east"])),
                )
                .unwrap(),
            )],
        )
        .unwrap();
        let partitioner = HashPartitioner::try_new(&schema, &["key".into()], 5).unwrap();
        let partition = |batch: &RecordBatch, value: &str| {
            partitioner
                .partition(batch)
                .unwrap()
                .iter()
                .position(|part| {
                    let values = arrow::compute::cast(part.column(0), &DataType::Utf8).unwrap();
                    values
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .iter()
                        .any(|candidate| candidate == Some(value))
                })
                .unwrap()
        };
        // The first row in each batch is a different dictionary index but the
        // same SQL value (`east`).
        assert_eq!(partition(&first, "east"), partition(&second, "east"));
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
