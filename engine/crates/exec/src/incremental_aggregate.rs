//! Incremental final-state merging. Encoded input is scratch, not retained state.
use std::collections::{HashMap, HashSet};

use arrow::array::{Array, BinaryArray};
use arrow::record_batch::RecordBatch;
use kaveon_core::{KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::aggregate::{
    AggregateState, AggregateValue, GroupedAggregateState, grouped_aggregate_key_types,
    grouped_aggregate_state_row, merge_grouped_aggregate_states,
};

pub struct IncrementalAggregateMerger {
    groups: HashMap<Vec<AggregateValue>, Vec<AggregateState>>,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
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
            groups: HashMap::new(),
            memory,
            reservations: ReservationSlab::default(),
        }
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
        for row in 0..batch.num_rows() {
            if let Some(memory) = &self.memory {
                memory.check_cancelled()?;
            }
            let partial = grouped_aggregate_state_row(keys, states, row, &types)?;
            let existing = self.groups.get(&partial.group_keys);
            let mut growth = if existing.is_none() {
                4096u64
                    .saturating_add(partial.group_keys.iter().map(value_bytes).sum::<u64>())
                    .saturating_add((partial.states.len() as u64).saturating_mul(512))
            } else {
                0
            };
            for (index, incoming) in partial.states.iter().enumerate() {
                if let Some(values) = distinct_values(incoming) {
                    let previous = existing
                        .and_then(|s| s.get(index))
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
            if let Some(existing) = self.groups.get_mut(&partial.group_keys) {
                if existing.len() != partial.states.len() {
                    return Err(error("aggregate state count mismatch"));
                }
                for (state, other) in existing.iter_mut().zip(&partial.states) {
                    state.merge(other)?;
                }
            } else {
                self.groups.insert(partial.group_keys, partial.states);
            }
        }
        Ok(())
    }

    /// Guards include headroom for canonical sorting and final output construction.
    pub fn finish(self) -> Result<(Vec<GroupedAggregateState>, Vec<MemoryReservation>)> {
        let groups = merge_grouped_aggregate_states(
            self.groups
                .into_iter()
                .map(|(group_keys, states)| GroupedAggregateState { group_keys, states }),
        )?;
        Ok((groups, self.reservations.into_guards()))
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

fn value_bytes(value: &AggregateValue) -> u64 {
    256u64.saturating_add(match value {
        AggregateValue::Utf8(value) => (value.len() as u64).saturating_mul(4),
        _ => 0,
    })
}

fn error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::grouped_aggregate_states_to_typed_batch;
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
    fn growing_distinct_state_fails_closed_and_releases_reservations() {
        let pool = QueryMemoryPool::new("distinct-growth", 256 * 1024).unwrap();
        let mut merger = IncrementalAggregateMerger::new(Some(pool.operator("final").unwrap()));
        let mut rejected = false;
        for value in 0..2000 {
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
}
