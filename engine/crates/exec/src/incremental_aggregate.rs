//! Incremental final-state merging. Encoded input is scratch, not retained state.
use std::collections::HashSet;

use ahash::AHashMap;
use arrow::array::{Array, BinaryArray};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use kaveon_core::{KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::aggregate::{
    AggregateState, AggregateValue, GroupedAggregateState, decode_group_keys,
    decode_group_states_into, grouped_aggregate_key_types, validate_group_key_types,
    validate_group_layouts,
};
use crate::columnar_aggregate::ColumnarGroups;

/// Groups are indexed by their encoded key bytes — the producer's canonical
/// per-value encoding — so a row costs one hash of those bytes and an
/// in-place merge into the group's accumulators; keys decode once, at the
/// end, per group rather than per row.
///
/// Shapes the columnar aggregate carries (integer, date, boolean and text
/// keys; in-place accumulators) merge into its columns instead: the encoded
/// key is parsed straight into key words, the states fold into flat
/// accumulator columns, and the final batch comes from those columns.
pub struct IncrementalAggregateMerger {
    index: AHashMap<Box<[u8]>, u32>,
    states: Vec<Vec<AggregateState>>,
    columnar: Columnar,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
}

/// Decided on the first row: the layout is the same for every row after.
enum Columnar {
    Undecided,
    Rows,
    Groups {
        groups: Box<ColumnarGroups>,
        slot_bytes: u64,
        growth_reserved_at: usize,
        /// The table's next doubling, replaced at each: only one is
        /// outstanding, since the old table is freed after the copy.
        growth: Option<MemoryReservation>,
    },
}

/// What the merge produced.
pub enum MergedGroups {
    Rows(Vec<GroupedAggregateState>),
    Columnar(Box<ColumnarGroups>),
}

const MERGE_RESERVATION_SLAB_BYTES: u64 = 64 * 1024;

#[derive(Default)]
struct ReservationSlab {
    guards: Vec<MemoryReservation>,
    available: u64,
}

impl ReservationSlab {
    fn reserve(&mut self, memory: &OperatorMemoryAccount, bytes: u64) -> Result<()> {
        if bytes > self.available {
            // Keep the common singleton-group case exact. Once a second group
            // proves that the merge is growing, amortize subsequent accounting.
            let slab_bytes = if self.guards.is_empty() {
                bytes
            } else {
                MERGE_RESERVATION_SLAB_BYTES.max(bytes)
            };
            let guard = match memory.reserve(slab_bytes) {
                Ok(guard) => guard,
                Err(KaveonError::MemoryLimit(_)) if slab_bytes != bytes => memory.reserve(bytes)?,
                Err(error) => return Err(error),
            };
            self.available = self.available.saturating_add(guard.bytes());
            self.guards.push(guard);
        }
        debug_assert!(bytes <= self.available);
        self.available -= bytes;
        Ok(())
    }

    fn into_guards(self) -> Vec<MemoryReservation> {
        self.guards
    }
}

impl IncrementalAggregateMerger {
    pub fn new(memory: Option<OperatorMemoryAccount>) -> Self {
        Self {
            index: AHashMap::new(),
            states: Vec::new(),
            columnar: Columnar::Undecided,
            memory,
            reservations: ReservationSlab::default(),
        }
    }

    /// The columnar merge for this batch's layout, when the columnar
    /// aggregate carries it. Decided from the first row; every later row
    /// has the same key types (checked by the caller) and state layout
    /// (checked on merge).
    fn decide(&mut self, types: &[DataType], first_states: &[AggregateState]) {
        if !matches!(self.columnar, Columnar::Undecided) {
            return;
        }
        self.columnar = match ColumnarGroups::new(types, first_states) {
            Some(groups) if !types.is_empty() => Columnar::Groups {
                slot_bytes: groups.slot_bytes(),
                groups: Box::new(groups),
                growth_reserved_at: 0,
                growth: None,
            },
            _ => Columnar::Rows,
        };
    }

    fn merge_columnar(
        &mut self,
        keys: &BinaryArray,
        states: &BinaryArray,
        rows: usize,
    ) -> Result<()> {
        let Columnar::Groups {
            groups,
            slot_bytes,
            growth_reserved_at,
            growth,
        } = &mut self.columnar
        else {
            return Err(error("columnar merge without columnar groups"));
        };
        let mut incoming = Vec::new();
        for row in 0..rows {
            if row % 1024 == 0
                && let Some(memory) = &self.memory
            {
                memory.check_cancelled()?;
            }
            if keys.is_null(row) || states.is_null(row) {
                return Err(error("grouped aggregate state row cannot contain nulls"));
            }
            decode_group_states_into(states.value(row), &mut incoming)?;
            if let Some(memory) = &self.memory {
                // A doubling is paid once per capacity, before it happens,
                // and released when the next one is paid.
                let doubling = groups.growth_bytes(1);
                if doubling != 0 && groups.capacity() != *growth_reserved_at {
                    drop(growth.take());
                    *growth = Some(memory.reserve(doubling)?);
                    *growth_reserved_at = groups.capacity();
                }
            }
            let (created, new_bytes) = groups.merge_encoded(keys.value(row), &incoming)?;
            if let Some(memory) = &self.memory {
                let bytes = if created { *slot_bytes } else { 0 }.saturating_add(new_bytes);
                if bytes != 0 {
                    self.reservations.reserve(memory, bytes)?;
                }
            }
        }
        Ok(())
    }

    pub fn push_batch(&mut self, batch: &RecordBatch) -> Result<()> {
        let _batch_guard = self
            .memory
            .as_ref()
            .map(|m| m.reserve(batch.get_array_memory_size() as u64))
            .transpose()?;
        // Validate empty batches too, and avoid downcast panics for invalid inputs.
        let types = grouped_aggregate_key_types(&batch.schema())?;
        let keys = batch
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .ok_or_else(|| error("invalid aggregate key column"))?;
        let states = batch
            .column(1)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .ok_or_else(|| error("invalid aggregate state column"))?;
        // Only one row is decoded at a time, so a single reservation for the
        // largest encoded row covers the whole batch without changing the
        // decoder's conservative memory bound.
        let scratch = (0..batch.num_rows()).try_fold(0_u64, |maximum, row| {
            let bytes = (keys.value_length(row) as u64)
                .saturating_add(states.value_length(row) as u64)
                .checked_mul(32)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| error("aggregate scratch estimate overflow"))?;
            Ok::<_, KaveonError>(maximum.max(bytes))
        })?;
        let _scratch_guard = self
            .memory
            .as_ref()
            .filter(|_| scratch != 0)
            .map(|memory| memory.reserve(scratch))
            .transpose()?;
        if batch.num_rows() == 0 {
            return Ok(());
        }
        if matches!(self.columnar, Columnar::Undecided) {
            if keys.is_null(0) || states.is_null(0) {
                return Err(error("grouped aggregate state row cannot contain nulls"));
            }
            let mut first = Vec::new();
            decode_group_states_into(states.value(0), &mut first)?;
            self.decide(&types, &first);
        }
        if matches!(self.columnar, Columnar::Groups { .. }) {
            return self.merge_columnar(keys, states, batch.num_rows());
        }
        let mut incoming = Vec::new();
        for row in 0..batch.num_rows() {
            if row % 1024 == 0
                && let Some(memory) = &self.memory
            {
                memory.check_cancelled()?;
            }
            if keys.is_null(row) || states.is_null(row) {
                return Err(error("grouped aggregate state row cannot contain nulls"));
            }
            let encoded_key = keys.value(row);
            decode_group_states_into(states.value(row), &mut incoming)?;
            let existing = self.index.get(encoded_key).copied();
            let mut growth = if existing.is_none() {
                NEW_GROUP_BYTES
                    .saturating_add(encoded_key.len() as u64)
                    .saturating_add((incoming.len() as u64).saturating_mul(STATE_BYTES))
            } else {
                0
            };
            for (position, state) in incoming.iter().enumerate() {
                if let Some(values) = distinct_values(state) {
                    let previous = existing
                        .and_then(|slot| self.states[slot as usize].get(position))
                        .and_then(distinct_values);
                    for value in values {
                        if previous.is_none_or(|p| !p.contains(value)) {
                            growth = growth.saturating_add(value_bytes(value));
                        }
                    }
                }
            }
            if growth != 0
                && let Some(memory) = &self.memory
            {
                self.reservations.reserve(memory, growth)?;
            }
            match existing {
                Some(slot) => {
                    let current = &mut self.states[slot as usize];
                    if current.len() != incoming.len() {
                        return Err(error("aggregate state count mismatch"));
                    }
                    for (state, other) in current.iter_mut().zip(&incoming) {
                        state.merge(other)?;
                    }
                }
                None => {
                    // The index and the state vector double when full; the
                    // old buffers live until the copy is done.
                    if let Some(memory) = &self.memory {
                        if self.index.capacity() >= 1 << 16
                            && self.index.len() == self.index.capacity()
                        {
                            self.reservations.reserve(
                                memory,
                                (self.index.capacity() as u64).saturating_mul(48),
                            )?;
                        }
                        if self.states.capacity() >= 1 << 16
                            && self.states.len() == self.states.capacity()
                        {
                            self.reservations.reserve(
                                memory,
                                (self.states.capacity() as u64).saturating_mul(24),
                            )?;
                        }
                    }
                    // The key's types are checked once, when the group is
                    // first seen: every later row with these bytes is the
                    // same key.
                    let group_keys = decode_group_keys(encoded_key)?;
                    validate_group_key_types(
                        std::slice::from_ref(&GroupedAggregateState {
                            group_keys,
                            states: Vec::new(),
                        }),
                        &types,
                    )?;
                    let slot = u32::try_from(self.states.len())
                        .map_err(|_| error("too many groups for one task"))?;
                    self.states.push(incoming.clone());
                    self.index.insert(Box::from(encoded_key), slot);
                }
            }
        }
        Ok(())
    }

    /// The merged groups in map order: the map made them unique, and the
    /// final output does not depend on their order.
    pub fn finish(self) -> Result<(Vec<GroupedAggregateState>, Vec<MemoryReservation>)> {
        let (groups, guards) = self.finish_groups()?;
        let groups = match groups {
            MergedGroups::Rows(groups) => groups,
            MergedGroups::Columnar(groups) => groups
                .into_groups()
                .into_iter()
                .map(|(keys, states)| GroupedAggregateState {
                    group_keys: keys.into_iter().map(AggregateValue::from).collect(),
                    states,
                })
                .collect(),
        };
        Ok((groups, guards))
    }

    /// The merged groups as they are held: columns when the columnar
    /// aggregate carried the layout, rows otherwise.
    pub fn finish_groups(self) -> Result<(MergedGroups, Vec<MemoryReservation>)> {
        if let Columnar::Groups { groups, growth, .. } = self.columnar {
            let mut guards = self.reservations.into_guards();
            guards.extend(growth);
            return Ok((MergedGroups::Columnar(groups), guards));
        }
        let mut keys: Vec<Option<Box<[u8]>>> = (0..self.states.len()).map(|_| None).collect();
        for (key, slot) in self.index {
            keys[slot as usize] = Some(key);
        }
        let groups = keys
            .into_iter()
            .zip(self.states)
            .map(|(key, states)| {
                Ok(GroupedAggregateState {
                    group_keys: decode_group_keys(&key.expect("every slot has its key"))?,
                    states,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        validate_group_layouts(&groups)?;
        Ok((MergedGroups::Rows(groups), self.reservations.into_guards()))
    }
}

fn distinct_values(state: &AggregateState) -> Option<&HashSet<AggregateValue>> {
    match state {
        AggregateState::CountDistinct(values)
        | AggregateState::SumDistinct(values)
        | AggregateState::AvgDistinct(values)
        | AggregateState::IntegerSumDistinct(values) => Some(values),
        AggregateState::Exact { distinct, .. } => distinct.as_ref(),
        _ => None,
    }
}

/// Map entry plus the two vectors' headers and hash overhead.
const NEW_GROUP_BYTES: u64 = 160;
/// One accumulator in place, with room for its enum payload.
const STATE_BYTES: u64 = 128;

fn value_bytes(value: &AggregateValue) -> u64 {
    48u64.saturating_add(match value {
        AggregateValue::Utf8(value) => (value.len() as u64).saturating_mul(2),
        _ => 0,
    })
}

fn error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{
        grouped_aggregate_states_to_typed_batch, merge_grouped_aggregate_states,
    };
    use arrow::datatypes::DataType;
    use kaveon_core::QueryMemoryPool;

    fn batch(state: AggregateState) -> RecordBatch {
        grouped_aggregate_states_to_typed_batch(
            &[GroupedAggregateState {
                group_keys: vec![AggregateValue::Null],
                states: vec![state],
            }],
            &[DataType::Int64],
        )
        .unwrap()
    }

    #[test]
    fn duplicate_scalar_and_distinct_states_do_not_accumulate_input_reservations() {
        for state in [
            AggregateState::Count(1),
            AggregateState::CountDistinct(HashSet::from([AggregateValue::Int64(7)])),
        ] {
            let pool = QueryMemoryPool::new("incremental", 256 * 1024).unwrap();
            let mut merger = IncrementalAggregateMerger::new(Some(pool.operator("final").unwrap()));
            let batch = batch(state.clone());
            for _ in 0..1000 {
                merger.push_batch(&batch).unwrap();
            }
            let (groups, guards) = merger.finish().unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].group_keys, vec![AggregateValue::Null]);
            assert_eq!(
                groups[0].states[0],
                match state {
                    AggregateState::Count(_) => AggregateState::Count(1000),
                    other => other,
                }
            );
            assert!(pool.snapshot().current_bytes < 8192);
            drop(groups);
            drop(guards);
            assert_eq!(pool.snapshot().current_bytes, 0);
        }
    }

    #[test]
    fn columnar_merge_matches_the_row_merge_and_holds_its_groups_as_columns() {
        // Integer, date and text keys with every in-place state: the columnar
        // merge takes the layout, folds partials from both encoders, and
        // finishes to the same groups the row merge produces.
        let states = |count: u64, sum: i128, text: Option<&str>| {
            vec![
                AggregateState::Count(count),
                AggregateState::IntegerSum { sum, count },
                AggregateState::Utf8Min(text.map(str::to_owned)),
                AggregateState::Min(Some(sum as f64)),
            ]
        };
        let key = |k: i64, d: i32, t: Option<&str>| {
            vec![
                AggregateValue::Int64(k),
                AggregateValue::Int32(d),
                t.map_or(AggregateValue::Null, |t| AggregateValue::Utf8(t.into())),
            ]
        };
        let types = [DataType::Int64, DataType::Date32, DataType::Utf8];
        let first = grouped_aggregate_states_to_typed_batch(
            &[
                GroupedAggregateState {
                    group_keys: key(1, 10, Some("a")),
                    states: states(2, 5, Some("m")),
                },
                GroupedAggregateState {
                    group_keys: key(2, 11, None),
                    states: states(1, -3, None),
                },
            ],
            &types,
        )
        .unwrap();
        let second = grouped_aggregate_states_to_typed_batch(
            &[
                GroupedAggregateState {
                    group_keys: key(1, 10, Some("a")),
                    states: states(3, 7, Some("b")),
                },
                GroupedAggregateState {
                    group_keys: key(1, 10, Some("z")),
                    states: states(1, 1, Some("q")),
                },
            ],
            &types,
        )
        .unwrap();
        let pool = QueryMemoryPool::new("columnar-merge", 16 * 1024 * 1024).unwrap();
        let mut merger = IncrementalAggregateMerger::new(Some(pool.operator("final").unwrap()));
        merger.push_batch(&first).unwrap();
        merger.push_batch(&second).unwrap();
        assert!(matches!(merger.columnar, Columnar::Groups { .. }));
        let (merged, guards) = merger.finish_groups().unwrap();
        let MergedGroups::Columnar(groups) = merged else {
            panic!("columnar layout merges into columns");
        };
        assert_eq!(groups.len(), 3);
        let keys = groups.key_arrays();
        assert_eq!(keys[1].data_type(), &DataType::Date32);
        let outputs = groups
            .output_arrays(&[
                DataType::UInt64,
                DataType::Int64,
                DataType::Utf8,
                DataType::Float64,
            ])
            .unwrap();
        let mut rows = (0..3)
            .map(|slot| {
                let (k, s) = groups.group(slot);
                let k = k.into_iter().map(AggregateValue::from).collect::<Vec<_>>();
                (format!("{k:?}"), format!("{s:?}"))
            })
            .collect::<Vec<_>>();
        rows.sort();
        let expected = merge_grouped_aggregate_states(
            [first.clone(), second.clone()]
                .iter()
                .flat_map(|b| {
                    crate::aggregate::grouped_aggregate_states_from_batches(std::slice::from_ref(b))
                        .unwrap()
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut expected = expected
            .iter()
            .map(|g| (format!("{:?}", g.group_keys), format!("{:?}", g.states)))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(rows, expected);
        assert_eq!(outputs[0].len(), 3);
        drop(groups);
        drop(guards);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn columnar_merge_rejects_a_key_of_another_type() {
        // The first row fixes the layout; a later row whose key bytes carry
        // another type fails closed instead of being read as bits.
        let types = [DataType::Int64];
        let first = grouped_aggregate_states_to_typed_batch(
            &[GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(1)],
                states: vec![AggregateState::Count(1)],
            }],
            &types,
        )
        .unwrap();
        let wrong = grouped_aggregate_states_to_typed_batch(
            &[GroupedAggregateState {
                group_keys: vec![AggregateValue::Int32(1)],
                states: vec![AggregateState::Count(1)],
            }],
            &[DataType::Int32],
        )
        .unwrap();
        let mut merger = IncrementalAggregateMerger::new(None);
        merger.push_batch(&first).unwrap();
        let wrong = RecordBatch::try_new(first.schema(), wrong.columns().to_vec()).unwrap();
        assert!(merger.push_batch(&wrong).is_err());
    }

    #[test]
    fn growing_distinct_state_fails_closed_and_releases_reservations() {
        let pool = QueryMemoryPool::new("distinct-growth", 256 * 1024).unwrap();
        let mut merger = IncrementalAggregateMerger::new(Some(pool.operator("final").unwrap()));
        let mut rejected = false;
        for value in 0..20_000 {
            let batch = batch(AggregateState::CountDistinct(HashSet::from([
                AggregateValue::Int64(value),
            ])));
            if merger.push_batch(&batch).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected);
        assert!(pool.snapshot().peak_bytes <= 256 * 1024);
        drop(merger);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn dense_group_batch_amortizes_scratch_and_growth_reservations() {
        let pool = QueryMemoryPool::new("dense-group-merge", 16 * 1024 * 1024).unwrap();
        let groups = (0..2_000)
            .map(|value| GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(value)],
                states: vec![AggregateState::Count(1)],
            })
            .collect::<Vec<_>>();
        let batch = grouped_aggregate_states_to_typed_batch(&groups, &[DataType::Int64]).unwrap();
        let mut merger = IncrementalAggregateMerger::new(Some(pool.operator("final").unwrap()));

        merger.push_batch(&batch).unwrap();
        let (merged, guards) = merger.finish().unwrap();

        assert_eq!(merged.len(), groups.len());
        assert!(
            merged
                .iter()
                .all(|group| group.states == vec![AggregateState::Count(1)])
        );
        assert!(
            merged
                .iter()
                .any(|group| { group.group_keys == vec![AggregateValue::Int64(0)] })
        );
        assert!(
            merged
                .iter()
                .any(|group| { group.group_keys == vec![AggregateValue::Int64(1_999)] })
        );
        assert!(pool.snapshot().reservation_calls < 200);
        assert!(pool.snapshot().peak_bytes <= 16 * 1024 * 1024);
        drop(guards);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn finish_matches_general_merge_without_rehashing_unique_groups() {
        let source = vec![
            GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(9)],
                states: vec![AggregateState::Count(2)],
            },
            GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(-4)],
                states: vec![AggregateState::Count(3)],
            },
            GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(9)],
                states: vec![AggregateState::Count(5)],
            },
        ];
        let expected = merge_grouped_aggregate_states(source.clone()).unwrap();
        let first =
            grouped_aggregate_states_to_typed_batch(&source[..2], &[DataType::Int64]).unwrap();
        let second =
            grouped_aggregate_states_to_typed_batch(&source[2..], &[DataType::Int64]).unwrap();
        let mut merger = IncrementalAggregateMerger::new(None);

        merger.push_batch(&first).unwrap();
        merger.push_batch(&second).unwrap();
        let (mut actual, guards) = merger.finish().unwrap();

        // Neither side orders its groups; compare them as sets.
        let mut expected = expected;
        let by_key = |group: &GroupedAggregateState| format!("{:?}", group.group_keys);
        actual.sort_by_key(by_key);
        expected.sort_by_key(by_key);
        assert_eq!(actual, expected);
        assert!(guards.is_empty());
    }
}
