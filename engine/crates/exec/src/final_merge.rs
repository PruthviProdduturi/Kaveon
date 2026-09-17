//! The final aggregate over one thread's share of partial rows: a hybrid
//! hash merge that never starts over.
//!
//! Partial rows merge into one in-memory table until the budget refuses
//! the next batch. The table's groups are then a valid partial for this
//! share of the keys, so they go to the disk as encoded partial rows — in
//! sub-partitions by a hash of the key, independent of the exchange's and
//! the thread's — and a fresh table takes the rest of the input, from the
//! row the refusal left off. Nothing is read twice from the input. Once
//! the input is drained: no run on disk, and the table is the result;
//! otherwise the table joins its sub-partitions on disk and each
//! sub-partition is merged back on its own, one at a time, as a unit of
//! complete groups. A sub-partition that does not fit fails the query
//! closed, the way every disk-partitioned operator does.
use std::collections::VecDeque;
use std::sync::Arc;

use ahash::RandomState;
use arrow::array::{Array, ArrayBuilder, BinaryArray, BinaryBuilder, BooleanArray};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::aggregate::{grouped_aggregate_key_types, grouped_aggregate_output_types};
use crate::incremental_aggregate::{IncrementalAggregateMerger, MergedGroups};
use crate::local_parallel::{RendezvousTicket, ThreadSelector};
use crate::partitioned::RunSource;
use crate::spill::{SpillManager, SpillRun, SpillRunWriter};

/// The sub-partitions nest inside the exchange's and the thread's
/// partitioning of the same keys; the salt keeps them independent of both.
const HYBRID_PARTITION_SALT: u64 = 0x2545_F491_4F6C_DD1D;
/// The most groups encoded at once when a table goes to the disk.
const SPILL_CHUNK_GROUPS: usize = 65_536;
/// Groups' worth of encoding held in reserve from the first batch on, so
/// a table that filled the budget can still leave for the disk: threads
/// sharing one budget reach their refusals together, and the one that
/// spills first would otherwise find nothing free to encode into.
const SPILL_RESERVE_GROUPS: u64 = 4_096;
/// Batches' worth prepaid to the merger's account from the first batch
/// on, so a fresh table after a spill can always take its next batch —
/// the batch held, its worst case ensured, its scratch, its groups —
/// whatever the sibling threads hold of the budget at that moment.
const PREPAID_BATCHES: u64 = 4;

/// What the merge spilled, for the record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FinalMergeSpill {
    /// Tables written to the disk: the refusals, plus the live table at
    /// the end when there were any.
    pub tables: u64,
    /// Groups those tables held.
    pub groups: u64,
}

pub struct HybridFinalMerge {
    source: Option<Box<dyn BatchOperator>>,
    schema: SchemaRef,
    group_types: Vec<DataType>,
    output_types: Vec<DataType>,
    memory: OperatorMemoryAccount,
    /// The mergers' account: `memory` with a prepaid balance once the
    /// first batch has shown a batch's size.
    merger_memory: Option<OperatorMemoryAccount>,
    spill: Option<(SpillManager, usize)>,
    hasher: RandomState,
    merger: Option<IncrementalAggregateMerger>,
    /// What a spill's chunk encodes into, held from the first batch on.
    spill_reserve: Option<MemoryReservation>,
    /// Where the merge waits for its sibling threads before emitting.
    rendezvous: Option<RendezvousTicket>,
    /// This merge sees every batch and keeps the rows whose key hashes
    /// to its thread.
    selection: Option<(ThreadSelector, usize)>,
    /// Runs on the disk by sub-partition, each a set of distinct groups.
    runs: Vec<Vec<SpillRun>>,
    /// Sub-partitions still to merge back once the input is drained.
    pending: VecDeque<usize>,
    drained: bool,
    failed: bool,
    spilled: FinalMergeSpill,
}

impl HybridFinalMerge {
    /// Over a grouped-state input. With `spill`, a budget refusal moves
    /// the groups held so far to the disk in that many sub-partitions;
    /// without, it is the query's error.
    pub fn new(
        source: Box<dyn BatchOperator>,
        memory: OperatorMemoryAccount,
        spill: Option<(SpillManager, usize)>,
    ) -> Result<Self> {
        let schema = Arc::clone(source.schema());
        let group_types = grouped_aggregate_key_types(&schema)?;
        let output_types = grouped_aggregate_output_types(&schema)?;
        if let Some((_, count)) = &spill
            && *count == 0
        {
            return Err(error("final merge needs at least one sub-partition"));
        }
        let partitions = spill.as_ref().map_or(0, |(_, count)| *count);
        Ok(Self {
            source: Some(source),
            schema,
            group_types,
            output_types,
            memory,
            merger_memory: None,
            spill,
            // Fixed seeds: every table this merge spills, and the live one
            // at the end, must agree on the sub-partition of a key.
            hasher: RandomState::with_seeds(
                0x243F_6A88_85A3_08D3,
                0x1319_8A2E_0370_7344,
                0xA409_3822_299F_31D0,
                0x082E_FA98_EC4E_6C89,
            ),
            merger: None,
            spill_reserve: None,
            rendezvous: None,
            selection: None,
            runs: (0..partitions).map(|_| Vec::new()).collect(),
            pending: VecDeque::new(),
            drained: false,
            failed: false,
            spilled: FinalMergeSpill::default(),
        })
    }

    /// One of several merges on the same budget: emit only once every
    /// one of them has finished merging.
    #[must_use]
    pub fn with_rendezvous(mut self, ticket: RendezvousTicket) -> Self {
        self.rendezvous = Some(ticket);
        self
    }

    /// Thread `index` of `workers` that each see every batch: keep the
    /// rows whose encoded key hashes to this thread.
    #[must_use]
    pub fn with_selection(mut self, index: usize, workers: usize) -> Self {
        self.selection = (workers > 1).then(|| (ThreadSelector::new(workers), index));
        self
    }

    /// The input's grouped-state schema.
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    pub fn spilled(&self) -> FinalMergeSpill {
        self.spilled
    }

    /// The next unit of complete groups — every group in it is final,
    /// and no later unit holds any of them — or None once every group is
    /// out. The whole merge as one unit when the budget held it; one
    /// sub-partition at a time otherwise.
    pub fn next_groups(&mut self) -> Result<Option<(MergedGroups, Vec<MemoryReservation>)>> {
        if self.failed {
            return Ok(None);
        }
        let memory = self.memory.clone();
        let result = crate::expr_eval::with_expression_memory(Some(&memory), || self.advance());
        if result.is_err() {
            self.failed = true;
            self.source = None;
            self.merger = None;
            self.merger_memory = None;
            self.spill_reserve = None;
            self.runs.clear();
            self.pending.clear();
        }
        result
    }

    fn advance(&mut self) -> Result<Option<(MergedGroups, Vec<MemoryReservation>)>> {
        if !self.drained {
            self.consume()?;
            self.drained = true;
            if let Some(ticket) = self.rendezvous.take() {
                ticket.wait(&self.memory)?;
            }
            let merger = self.merger.take();
            if self.runs.iter().all(Vec::is_empty) {
                self.spill_reserve = None;
                return merger.map(Self::finished).transpose();
            }
            // The live groups are the last partial of this share: they
            // join their sub-partitions, and the memory is the disk's
            // until each comes back.
            if let Some(merger) = merger
                && merger.group_count() > 0
            {
                self.spill_table(merger)?;
            }
            // Nothing spills from here on.
            self.spill_reserve = None;
            self.pending = (0..self.runs.len())
                .filter(|partition| !self.runs[*partition].is_empty())
                .collect();
        }
        let Some(partition) = self.pending.pop_front() else {
            // Every unit is out: the prepaid balance goes back with the
            // last reservation that drew on it.
            self.merger_memory = None;
            return Ok(None);
        };
        let runs = std::mem::take(&mut self.runs[partition]);
        let memory = self
            .merger_memory
            .clone()
            .unwrap_or_else(|| self.memory.clone());
        let mut merger = IncrementalAggregateMerger::new(Some(memory));
        let mut source = RunSource::new(Arc::clone(&self.schema), runs);
        while let Some(batch) = source.next_batch()? {
            merger.push_batch(&batch).map_err(|error| match error {
                KaveonError::MemoryLimit(message) => KaveonError::MemoryLimit(format!(
                    "final aggregate sub-partition {} of {} does not fit the budget: {message}",
                    partition + 1,
                    self.runs.len()
                )),
                error => error,
            })?;
        }
        drop(source);
        Self::finished(merger).map(Some)
    }

    /// A merge's groups as a unit: the table will not grow again, so the
    /// doubling it held in reserve goes back to the budget before the
    /// unit is emitted — the headroom the batches built from it, and
    /// whatever runs over them, take.
    fn finished(
        mut merger: IncrementalAggregateMerger,
    ) -> Result<(MergedGroups, Vec<MemoryReservation>)> {
        merger.release_growth();
        merger.finish_groups()
    }

    /// Every input batch into the table, spilling the table whenever the
    /// budget refuses one.
    fn consume(&mut self) -> Result<()> {
        let Some(mut source) = self.source.take() else {
            return Ok(());
        };
        while let Some(batch) = source.next_batch()? {
            if grouped_aggregate_key_types(&batch.schema())? != self.group_types {
                return Err(error("final aggregate input key schema changed"));
            }
            if grouped_aggregate_output_types(&batch.schema())? != self.output_types {
                return Err(error("final aggregate input output schema changed"));
            }
            let batch = match &self.selection {
                Some((selector, index)) => {
                    let Some(batch) = select_rows(&batch, selector, *index)? else {
                        continue;
                    };
                    batch
                }
                None => batch,
            };
            self.push(batch)?;
        }
        Ok(())
    }

    fn push(&mut self, mut batch: RecordBatch) -> Result<()> {
        if self.spill.is_some() && self.spill_reserve.is_none() && batch.num_rows() > 0 {
            // The input rows are the encoding a spilled group takes.
            let bytes_per_row = encoded_row_bytes(&batch)?;
            self.spill_reserve = Some(
                self.memory
                    .reserve(SPILL_RESERVE_GROUPS.saturating_mul(bytes_per_row * 3))?,
            );
        }
        if self.merger_memory.is_none() {
            let prepaid = if self.spill.is_some() {
                (batch.get_array_memory_size() as u64).saturating_mul(PREPAID_BATCHES)
            } else {
                0
            };
            self.merger_memory = Some(self.memory.prepaid(prepaid)?);
        }
        loop {
            let memory = self.merger_memory.as_ref().expect("set above");
            let merger = self
                .merger
                .get_or_insert_with(|| IncrementalAggregateMerger::new(Some(memory.clone())));
            let message = match merger.push_batch(&batch) {
                Ok(()) => return Ok(()),
                Err(KaveonError::MemoryLimit(message)) => message,
                Err(error) => return Err(error),
            };
            // Nothing to make room with: the batch alone is over the
            // budget, or there is no disk.
            if self.spill.is_none() || merger.group_count() == 0 {
                return Err(KaveonError::MemoryLimit(message));
            }
            let applied = merger.rows_applied();
            let merger = self.merger.take().expect("held above");
            self.spill_table(merger)?;
            if applied >= batch.num_rows() {
                return Ok(());
            }
            batch = batch.slice(applied, batch.num_rows() - applied);
        }
    }

    /// The table's groups to the disk as encoded partial rows, one run
    /// per sub-partition they hash to, in chunks the budget admits beside
    /// the table; the table is gone when this returns.
    fn spill_table(&mut self, mut merger: IncrementalAggregateMerger) -> Result<()> {
        let (spill, count) = self.spill.clone().expect("spilling needs a spill");
        merger.release_growth();
        let (merged, reservations) = merger.finish_groups()?;
        let total = merged.len();
        // Per group, the encoded key and states: estimated for the first
        // chunk, measured on it for the rest, with room for either to be
        // short. A chunk costs twice its encoding — the batch, and the IPC
        // writer's copy of it — and comes out of the spill reserve, plus
        // the budget's headroom when it has some: a table that filled the
        // budget goes in the reserve's chunks, one with room to spare in
        // large ones.
        let mut bytes_per_group = merged
            .encoded_bytes(&reservations)
            .div_ceil(total.max(1) as u64)
            .saturating_mul(3)
            .div_ceil(2)
            .max(1);
        let hasher = &self.hasher;
        let partition_of = move |key: &[u8]| -> usize {
            (crate::exchange::mix(hasher.hash_one(key) ^ HYBRID_PARTITION_SALT) % count as u64)
                as usize
        };
        let mut writers: Vec<Option<SpillRunWriter>> = (0..count).map(|_| None).collect();
        let reserved = self
            .spill_reserve
            .as_ref()
            .map_or(0, MemoryReservation::bytes);
        let mut start = 0usize;
        while start < total {
            self.memory.check_cancelled()?;
            // The chunk the reserve covers, or a larger one when the
            // budget has room to spare for the difference.
            let chunk_bytes_per_group = bytes_per_group.saturating_mul(2);
            let covered = usize::try_from(reserved / chunk_bytes_per_group).unwrap_or(usize::MAX);
            let snapshot = self.memory.query().snapshot();
            let headroom = snapshot.limit_bytes.saturating_sub(snapshot.current_bytes);
            let mut chunk_groups =
                usize::try_from((reserved.saturating_add(headroom / 2)) / chunk_bytes_per_group)
                    .unwrap_or(SPILL_CHUNK_GROUPS)
                    .clamp(1, SPILL_CHUNK_GROUPS)
                    .min(total - start);
            let extra = (chunk_groups as u64)
                .saturating_mul(chunk_bytes_per_group)
                .saturating_sub(reserved);
            let _extra = if extra > 0 {
                match self.memory.reserve(extra) {
                    Ok(guard) => Some(guard),
                    Err(KaveonError::MemoryLimit(_)) if covered > 0 => {
                        chunk_groups = covered.min(total - start);
                        None
                    }
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            let end = start + chunk_groups;
            let sink_capacity = (end - start).div_ceil(count);
            let mut sinks = (0..count)
                .map(|_| {
                    (
                        BinaryBuilder::with_capacity(
                            sink_capacity,
                            sink_capacity * bytes_per_group as usize / 2,
                        ),
                        BinaryBuilder::with_capacity(
                            sink_capacity,
                            sink_capacity * bytes_per_group as usize / 2,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            merged.encode_partitioned(start..end, &partition_of, &mut sinks)?;
            let mut encoded = 0u64;
            for (partition, (mut keys, mut states)) in sinks.into_iter().enumerate() {
                if keys.is_empty() {
                    continue;
                }
                // What the rows took, not what the builders hold in reserve.
                encoded = encoded
                    .saturating_add(keys.values_slice().len() as u64)
                    .saturating_add(states.values_slice().len() as u64)
                    .saturating_add((keys.len() as u64 + 1).saturating_mul(8));
                let batch = RecordBatch::try_new(
                    Arc::clone(&self.schema),
                    vec![Arc::new(keys.finish()), Arc::new(states.finish())],
                )?;
                let writer = match &mut writers[partition] {
                    Some(writer) => writer,
                    slot => slot.insert(spill.begin_run(&self.schema)?),
                };
                writer.write(&batch)?;
            }
            bytes_per_group = encoded
                .div_ceil((end - start) as u64)
                .saturating_mul(3)
                .div_ceil(2)
                .max(1);
            start = end;
        }
        drop(merged);
        drop(reservations);
        for (partition, writer) in writers.into_iter().enumerate() {
            if let Some(writer) = writer {
                self.runs[partition].push(writer.finish()?);
            }
        }
        self.spilled.tables += 1;
        self.spilled.groups += total as u64;
        Ok(())
    }
}

/// The rows of a grouped-state batch whose key hashes to thread `index`,
/// or None when there are none; the batch itself when they all do.
fn select_rows(
    batch: &RecordBatch,
    selector: &ThreadSelector,
    index: usize,
) -> Result<Option<RecordBatch>> {
    let keys = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| error("invalid aggregate key column"))?;
    let mut kept = 0usize;
    let mask = BooleanArray::from_iter((0..batch.num_rows()).map(|row| {
        let mine = !keys.is_null(row) && selector.thread_of(keys.value(row)) == index;
        kept += usize::from(mine);
        Some(mine)
    }));
    if kept == 0 {
        return Ok(None);
    }
    if kept == batch.num_rows() {
        return Ok(Some(batch.clone()));
    }
    Ok(Some(arrow::compute::filter_record_batch(batch, &mask)?))
}

/// Bytes per row of a grouped-state batch: its key and state bytes with
/// their offsets, over its rows.
fn encoded_row_bytes(batch: &RecordBatch) -> Result<u64> {
    let bytes = batch
        .columns()
        .iter()
        .map(|column| {
            let column = column
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or_else(|| error("grouped-state batch columns are binary"))?;
            let offsets = column.offsets();
            Ok((offsets[offsets.len() - 1] - offsets[0]) as u64
                + (offsets.len() as u64).saturating_mul(4))
        })
        .sum::<Result<u64>>()?;
    Ok(bytes.div_ceil(batch.num_rows().max(1) as u64).max(1))
}

fn error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Array, Int64Array, UInt64Array};
    use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{BatchOperator, KaveonError, QueryMemoryPool, Result};

    use super::HybridFinalMerge;
    use crate::aggregate::{
        AggregateState, AggregateValue, GroupedAggregateState,
        grouped_aggregate_states_to_typed_batch,
    };
    use crate::incremental_aggregate::{IncrementalAggregateMerger, MergedGroups};
    use crate::local_parallel::{
        ParallelPartials, Rendezvous, SourceOpener, Sources, ThreadContext, ThreadOperator,
    };
    use crate::spill::SpillManager;

    /// The q33 shape: `GROUP BY WatchID, ClientIP` with `COUNT(*)`,
    /// `SUM(IsRefresh)` and `AVG(ResolutionWidth)` — Int64 + Int32 keys,
    /// near unique (one row in sixteen repeats the key fifteen rows
    /// before it), three columnar states.
    const REPEAT_EVERY: usize = 16;
    const KEY_TYPES: [DataType; 2] = [DataType::Int64, DataType::Int32];
    const OUTPUT_TYPES: [DataType; 3] = [DataType::UInt64, DataType::Int64, DataType::Float64];

    fn q33_partial_batches(rows: usize, batch_rows: usize) -> Vec<RecordBatch> {
        let source_row = |i: usize| {
            if i % REPEAT_EVERY == REPEAT_EVERY - 1 {
                i - (REPEAT_EVERY - 1)
            } else {
                i
            }
        };
        (0..rows.div_ceil(batch_rows))
            .map(|batch| {
                let groups = (batch * batch_rows..((batch + 1) * batch_rows).min(rows))
                    .map(source_row)
                    .map(|i| GroupedAggregateState {
                        group_keys: vec![
                            AggregateValue::Int64(
                                (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) as i64
                            ),
                            AggregateValue::Int32((i as u32).wrapping_mul(0x85EB_CA6B) as i32),
                        ],
                        states: vec![
                            AggregateState::Count(1),
                            AggregateState::IntegerSum {
                                sum: i128::from(i.is_multiple_of(3)),
                                count: 1,
                            },
                            AggregateState::Avg {
                                sum: 1000.0 + (i % 500) as f64,
                                count: 1,
                            },
                        ],
                    })
                    .collect::<Vec<_>>();
                grouped_aggregate_states_to_typed_batch(&groups, &KEY_TYPES).unwrap()
            })
            .collect()
    }

    struct Batches {
        schema: SchemaRef,
        batches: std::collections::VecDeque<RecordBatch>,
    }
    impl BatchOperator for Batches {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    fn batches(batches: &[RecordBatch]) -> Box<dyn BatchOperator> {
        Box::new(Batches {
            schema: batches[0].schema(),
            batches: batches.iter().cloned().collect(),
        })
    }

    /// The finalised batch of the merged groups, the way the fragment
    /// executor builds it: keys as the exchange typed them, outputs from
    /// the accumulator columns.
    fn finalized(merged: MergedGroups) -> Result<RecordBatch> {
        let MergedGroups::Columnar(groups) = merged else {
            return Err(KaveonError::Execution(
                "expected the columnar layout".into(),
            ));
        };
        let (keys, outputs) = groups.into_final_arrays(&OUTPUT_TYPES)?;
        let fields = keys
            .iter()
            .chain(&outputs)
            .enumerate()
            .map(|(i, column)| Field::new(format!("c{i}"), column.data_type().clone(), true))
            .collect::<Vec<_>>();
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            keys.into_iter().chain(outputs).collect(),
        )?)
    }

    fn final_schema() -> SchemaRef {
        let groups = crate::columnar_aggregate::ColumnarGroups::new(
            &KEY_TYPES,
            &[
                AggregateState::Count(0),
                AggregateState::IntegerSum { sum: 0, count: 0 },
                AggregateState::Avg { sum: 0.0, count: 0 },
            ],
        )
        .unwrap();
        finalized(MergedGroups::Columnar(Box::new(groups)))
            .unwrap()
            .schema()
    }

    /// The in-memory merge over a source, finalised as one batch: what
    /// each thread of the replayable final ran before the hybrid merge.
    fn in_memory_final(
        mut source: Box<dyn BatchOperator>,
        pool: &QueryMemoryPool,
    ) -> Result<Box<dyn BatchOperator>> {
        let account = pool.operator("final-aggregate")?;
        let mut merger = IncrementalAggregateMerger::new(Some(account.clone()));
        while let Some(batch) = source.next_batch()? {
            merger.push_batch(&batch)?;
        }
        let (merged, reservations) = merger.finish_groups()?;
        let batch = finalized(merged)?;
        drop(reservations);
        let _held = account.reserve(batch.get_array_memory_size() as u64)?;
        Ok(Box::new(Batches {
            schema: batch.schema(),
            batches: std::collections::VecDeque::from([batch]),
        }))
    }

    /// The hybrid merge over a source, each unit of complete groups
    /// finalised in batches of `OUTPUT_ROWS` rows built from the unit's
    /// columns while they stay whole: what each thread runs now, the way
    /// the fragment executor emits it.
    const OUTPUT_ROWS: usize = 4_096;
    struct HybridFinal {
        merge: HybridFinalMerge,
        schema: SchemaRef,
        account: kaveon_core::OperatorMemoryAccount,
        current: Option<(
            Box<crate::columnar_aggregate::ColumnarGroups>,
            Vec<kaveon_core::MemoryReservation>,
            usize,
        )>,
        held: Option<kaveon_core::MemoryReservation>,
    }
    impl BatchOperator for HybridFinal {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            self.held = None;
            loop {
                if let Some((groups, _, offset)) = &mut self.current
                    && *offset < groups.len()
                {
                    let end = (*offset + OUTPUT_ROWS).min(groups.len());
                    let (keys, outputs) = groups.final_arrays(*offset..end, &OUTPUT_TYPES)?;
                    *offset = end;
                    let batch = RecordBatch::try_new(
                        self.schema.clone(),
                        keys.into_iter().chain(outputs).collect(),
                    )?;
                    self.held = Some(self.account.reserve(batch.get_array_memory_size() as u64)?);
                    return Ok(Some(batch));
                }
                self.current = None;
                let Some((merged, reservations)) = self.merge.next_groups()? else {
                    return Ok(None);
                };
                let MergedGroups::Columnar(groups) = merged else {
                    return Err(KaveonError::Execution(
                        "expected the columnar layout".into(),
                    ));
                };
                self.current = Some((groups, reservations, 0));
            }
        }
    }
    fn hybrid_final(
        source: Box<dyn BatchOperator>,
        pool: &QueryMemoryPool,
        spill: Option<(SpillManager, usize)>,
    ) -> Result<Box<dyn BatchOperator>> {
        let account = pool.operator("final-aggregate")?;
        Ok(Box::new(HybridFinal {
            merge: HybridFinalMerge::new(source, account.clone(), spill)?,
            schema: final_schema(),
            account,
            current: None,
            held: None,
        }))
    }

    /// The hybrid merge as one of `workers` threads that each see every
    /// batch: keeps its own rows, waits for the others before emitting.
    fn hybrid_final_of(
        source: Box<dyn BatchOperator>,
        pool: &QueryMemoryPool,
        context: &ThreadContext,
        rendezvous: &Arc<Rendezvous>,
    ) -> Result<Box<dyn BatchOperator>> {
        let account = pool.operator("final-aggregate")?;
        Ok(Box::new(HybridFinal {
            merge: HybridFinalMerge::new(source, account.clone(), context.spill.clone())?
                .with_selection(context.index, context.workers)
                .with_rendezvous(rendezvous.ticket()),
            schema: final_schema(),
            account,
            current: None,
            held: None,
        }))
    }

    /// The batches as `count` Arrow IPC streams, the way an exchange holds
    /// one spooled payload per producer.
    fn ipc_payloads(batches_all: &[RecordBatch], count: usize) -> Vec<Arc<[u8]>> {
        let per_payload = batches_all.len().div_ceil(count);
        batches_all
            .chunks(per_payload)
            .map(|chunk| {
                let mut bytes = Vec::new();
                let mut writer =
                    arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &chunk[0].schema())
                        .unwrap();
                for batch in chunk {
                    writer.write(batch).unwrap();
                }
                writer.finish().unwrap();
                Arc::from(bytes)
            })
            .collect()
    }

    /// One payload decoded batch by batch, as the exchange input decodes
    /// its spool.
    struct IpcSource {
        schema: SchemaRef,
        reader: arrow::ipc::reader::StreamReader<std::io::Cursor<Arc<[u8]>>>,
    }
    impl BatchOperator for IpcSource {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.reader.next().transpose()?)
        }
    }
    fn ipc_source(payload: &Arc<[u8]>) -> Box<dyn BatchOperator> {
        let reader = arrow::ipc::reader::StreamReader::try_new(
            std::io::Cursor::new(Arc::clone(payload)),
            None,
        )
        .unwrap();
        Box::new(IpcSource {
            schema: reader.schema(),
            reader,
        })
    }

    /// Every payload decoded on the calling thread, one after another.
    fn ipc_sources_here(payloads: &[Arc<[u8]>]) -> Box<dyn BatchOperator> {
        Sources::Threads {
            schema: ipc_source(&payloads[0]).schema().clone(),
            openers: payloads
                .iter()
                .map(|payload| {
                    let payload = Arc::clone(payload);
                    Box::new(move || Ok(ipc_source(&payload))) as SourceOpener
                })
                .collect(),
        }
        .into_operator()
        .unwrap()
    }

    /// Every payload decoded on a thread of its own.
    fn ipc_sources_threads(payloads: &[Arc<[u8]>]) -> Sources {
        Sources::Threads {
            schema: ipc_source(&payloads[0]).schema().clone(),
            openers: payloads
                .iter()
                .map(|payload| {
                    let payload = Arc::clone(payload);
                    Box::new(move || Ok(ipc_source(&payload))) as SourceOpener
                })
                .collect(),
        }
    }

    #[derive(Debug)]
    struct Totals {
        rows: usize,
        count: u64,
        sum: i64,
        batches: usize,
    }
    fn totals(operator: &mut dyn BatchOperator) -> Result<Totals> {
        let mut totals = Totals {
            rows: 0,
            count: 0,
            sum: 0,
            batches: 0,
        };
        while let Some(batch) = operator.next_batch()? {
            totals.batches += 1;
            totals.rows += batch.num_rows();
            totals.count += batch
                .column(2)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<u64>();
            totals.sum += batch
                .column(3)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<i64>();
        }
        Ok(totals)
    }

    fn spill_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kaveon-final-merge-{}-{name}", std::process::id()))
    }

    fn keys_of(batch: &RecordBatch) -> Vec<(i64, i32)> {
        let a = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let b = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow::array::Int32Array>()
            .unwrap();
        (0..batch.num_rows())
            .map(|row| (a.value(row), b.value(row)))
            .collect()
    }

    #[test]
    fn hybrid_merge_holds_what_fits_and_is_exact_without_a_spill() {
        let batches_all = q33_partial_batches(50_000, 4_096);
        let pool = QueryMemoryPool::new("fits", 64 << 20).unwrap();
        let mut merged = hybrid_final(batches(&batches_all), &pool, None).unwrap();
        let totals = totals(&mut *merged).unwrap();
        assert_eq!(totals.batches, totals.rows.div_ceil(OUTPUT_ROWS));
        assert_eq!(totals.rows, 50_000 - 50_000 / REPEAT_EVERY);
        assert_eq!(totals.count, 50_000);
        drop(merged);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn hybrid_merge_without_a_spill_fails_closed_at_the_budget() {
        let batches_all = q33_partial_batches(200_000, 8_192);
        let pool = QueryMemoryPool::new("no-disk", 6 << 20).unwrap();
        let mut merged = hybrid_final(batches(&batches_all), &pool, None).unwrap();
        let error = totals(&mut *merged).unwrap_err();
        assert!(matches!(error, KaveonError::MemoryLimit(_)), "{error}");
        assert!(merged.next_batch().unwrap().is_none());
        drop(merged);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn hybrid_merge_spills_on_refusal_and_merges_every_sub_partition_back_exactly() {
        const ROWS: usize = 400_000;
        let batches_all = q33_partial_batches(ROWS, 8_192);
        let expected = {
            let mut serial = in_memory_final(
                batches(&batches_all),
                &QueryMemoryPool::new("reference", 1 << 30).unwrap(),
            )
            .unwrap();
            let batch = serial.next_batch().unwrap().unwrap();
            let mut keys = keys_of(&batch);
            keys.sort_unstable();
            keys
        };
        let pool = QueryMemoryPool::new("spills", 12 << 20).unwrap();
        let spill = SpillManager::new(spill_root("spills"), 1 << 30).unwrap();
        let mut merged =
            hybrid_final(batches(&batches_all), &pool, Some((spill.clone(), 8))).unwrap();
        let mut keys = Vec::new();
        let mut count = 0;
        let mut sum = 0;
        let mut batches_out = 0;
        while let Some(batch) = merged.next_batch().unwrap() {
            batches_out += 1;
            keys.extend(keys_of(&batch));
            count += batch
                .column(2)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<u64>();
            sum += batch
                .column(3)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .iter()
                .sum::<i64>();
            assert!(pool.snapshot().peak_bytes <= 12 << 20);
        }
        // Every group once, across the sub-partitions, with the counts of
        // the serial merge.
        keys.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(count, ROWS as u64);
        assert_eq!(
            sum,
            (0..ROWS).filter(|i| i.is_multiple_of(3)).count() as i64
        );
        assert!(batches_out > 1, "{batches_out}");
        let snapshot = spill.snapshot();
        assert!(snapshot.runs_written > 0);
        assert_eq!(snapshot.compactions, 0);
        assert_eq!(snapshot.current_bytes, 0, "the runs are gone");
        drop(merged);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn hybrid_merge_resumes_a_refused_batch_from_the_row_it_stopped_at() {
        // The row layout (a DISTINCT state) merges row by row, so a
        // refusal lands inside a batch: the rows before it are in the
        // spilled table, the rows from it on go to the fresh one.
        let rows = (0..60_000i64)
            .map(|i| GroupedAggregateState {
                group_keys: vec![AggregateValue::Int64(i / 2)],
                states: vec![AggregateState::CountDistinct(
                    [AggregateValue::Int64(i % 3)].into_iter().collect(),
                )],
            })
            .collect::<Vec<_>>();
        let batches_all = rows
            .chunks(5_000)
            .map(|chunk| {
                grouped_aggregate_states_to_typed_batch(chunk, &[DataType::Int64]).unwrap()
            })
            .collect::<Vec<_>>();
        let pool = QueryMemoryPool::new("rows", 4 << 20).unwrap();
        let spill = SpillManager::new(spill_root("rows"), 1 << 30).unwrap();
        let mut merge = HybridFinalMerge::new(
            batches(&batches_all),
            pool.operator("final").unwrap(),
            Some((spill.clone(), 8)),
        )
        .unwrap();
        let mut groups = 0usize;
        let mut distinct = 0usize;
        while let Some((merged, reservations)) = merge.next_groups().unwrap() {
            let MergedGroups::Rows(merged) = merged else {
                panic!("DISTINCT states merge on the row path");
            };
            groups += merged.len();
            for group in &merged {
                let AggregateState::CountDistinct(values) = &group.states[0] else {
                    panic!("layout");
                };
                distinct += values.len();
            }
            drop(reservations);
        }
        assert_eq!(groups, 30_000);
        // Key k has rows 2k and 2k+1: values (2k % 3, (2k+1) % 3) — always
        // two distinct values.
        assert_eq!(distinct, 60_000);
        assert!(merge.spilled().tables >= 2, "{:?}", merge.spilled());
        assert_eq!(spill.snapshot().current_bytes, 0);
        drop(merge);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn hybrid_merge_fails_closed_when_a_sub_partition_does_not_fit() {
        let batches_all = q33_partial_batches(400_000, 8_192);
        let pool = QueryMemoryPool::new("skew", 6 << 20).unwrap();
        let spill = SpillManager::new(spill_root("skew"), 1 << 30).unwrap();
        // One sub-partition: everything spilled comes back at once.
        let mut merged =
            hybrid_final(batches(&batches_all), &pool, Some((spill.clone(), 1))).unwrap();
        let error = totals(&mut *merged).unwrap_err();
        assert!(
            matches!(&error, KaveonError::MemoryLimit(message) if message.contains("sub-partition 1 of 1")),
            "{error}"
        );
        assert!(merged.next_batch().unwrap().is_none());
        drop(merged);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    /// The final stage on the q33 shape under a budget that refuses the
    /// in-memory merge near the end of its input, with a spill registered:
    /// the path the AKS final stage takes on ClickBench q19/q33/q34/q35.
    /// Before: the refused attempt thrown away and the input replayed
    /// through the partitioned disk path. After: the hybrid merge.
    /// Ignored by default; run it as
    /// `cargo test --release -p kaveon-exec final_merge_under_pressure -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark: prints the final merge figures, run explicitly in release"]
    fn final_merge_under_pressure() {
        const ROWS: usize = 6_000_000;
        const BATCH_ROWS: usize = 262_144;
        const THREADS: usize = 3;
        let build_started = std::time::Instant::now();
        let batches_all = q33_partial_batches(ROWS, BATCH_ROWS);
        let encoded_bytes = batches_all
            .iter()
            .map(RecordBatch::get_array_memory_size)
            .sum::<usize>();
        println!(
            "built {ROWS} partial rows ({} MiB encoded) in {:.2?}",
            encoded_bytes >> 20,
            build_started.elapsed()
        );
        let expected_groups = ROWS - ROWS / REPEAT_EVERY;
        let expected_sum = (0..ROWS).filter(|i| i.is_multiple_of(3)).count() as i64;
        // Three producers' payloads, as the exchange spools them.
        const PRODUCERS: usize = 3;
        let payloads = ipc_payloads(&batches_all, PRODUCERS);
        println!(
            "{PRODUCERS} IPC payloads of {} MiB",
            payloads.iter().map(|p| p.len()).sum::<usize>() >> 20
        );

        // The pump alone: the calling thread decodes every payload and
        // partitions every batch by the thread's hash before any merge
        // thread sees a row.
        {
            let started = std::time::Instant::now();
            let mut source = ipc_sources_here(&payloads);
            let mut decoded = 0usize;
            while let Some(batch) = source.next_batch().unwrap() {
                decoded += batch.num_rows();
            }
            let decode = started.elapsed();
            let partitioner = crate::exchange::HashPartitioner::try_new_salted(
                &batches_all[0].schema(),
                &["group_keys".into()],
                THREADS,
                crate::exchange::THREAD_PARTITION_SALT,
            )
            .unwrap();
            let started = std::time::Instant::now();
            let mut parts = 0usize;
            for batch in &batches_all {
                parts += partitioner.partition(batch).unwrap().len();
            }
            println!(
                "pump alone: {decode:.2?} to decode {decoded} rows ({:.0} ns/row), {:.2?} to \
                 partition them into {parts} parts ({:.0} ns/row)",
                decode.as_nanos() as f64 / ROWS as f64,
                started.elapsed(),
                started.elapsed().as_nanos() as f64 / ROWS as f64
            );
        }

        // The budget: the merge on three threads holds about 80 % of the
        // groups before a doubling is refused.
        let budget = 640u64 << 20;
        for round in 1..=3 {
            let pool = QueryMemoryPool::new("final-before", budget).unwrap();
            let spill = SpillManager::new(spill_root("before"), 8 << 30).unwrap();
            pool.shared_resource("kaveon.exec.hash-spill.v1", || Ok((spill.clone(), 16usize)))
                .unwrap();
            let started = std::time::Instant::now();
            let operator: ThreadOperator =
                Arc::new(move |source, pool, _| in_memory_final(source, pool));
            let mut attempt = ParallelPartials::partitioned(
                ipc_sources_here(&payloads),
                final_schema(),
                vec!["group_keys".into()],
                operator,
                pool.clone(),
                THREADS,
            )
            .unwrap();
            let refused = match totals(&mut attempt) {
                Ok(_) => panic!("the budget is meant to refuse the in-memory attempt"),
                Err(KaveonError::MemoryLimit(message)) => message,
                Err(error) => panic!("{error}"),
            };
            drop(attempt);
            let attempt_took = started.elapsed();
            assert_eq!(pool.snapshot().current_bytes, 0);
            // The replay: the reopened input through the partitioned disk
            // path, one partition merged at a time on the calling thread.
            let replay_started = std::time::Instant::now();
            let partitions = crate::partitioned::partition_sources(
                ipc_sources_here(&payloads),
                &["group_keys".into()],
                16,
                &pool.operator("final-partition").unwrap(),
                &spill,
            )
            .unwrap();
            let partitioned_in = replay_started.elapsed();
            let mut totals_all = Totals {
                rows: 0,
                count: 0,
                sum: 0,
                batches: 0,
            };
            for partition in partitions {
                let mut merged = in_memory_final(partition, &pool).unwrap();
                let part = totals(&mut *merged).unwrap();
                totals_all.rows += part.rows;
                totals_all.count += part.count;
                totals_all.sum += part.sum;
            }
            let total = started.elapsed();
            assert_eq!(totals_all.rows, expected_groups);
            assert_eq!(totals_all.count, ROWS as u64);
            assert_eq!(totals_all.sum, expected_sum);
            let snapshot = spill.snapshot();
            let memory = pool.snapshot();
            println!(
                "before, round {round}: {total:.2?} wall (attempt {attempt_took:.2?} refused: \
                 {refused}; replay partition {partitioned_in:.2?}, merge {:.2?}); spill {} MiB \
                 written in {} runs, {} compactions over {} MiB, write {:.2?} read {:.2?}; \
                 memory peak {} MiB, {} reservation calls",
                total - attempt_took - partitioned_in,
                snapshot.bytes_written >> 20,
                snapshot.runs_written,
                snapshot.compactions,
                snapshot.compaction_input_bytes >> 20,
                std::time::Duration::from_micros(snapshot.write_us),
                std::time::Duration::from_micros(snapshot.read_us),
                memory.peak_bytes >> 20,
                memory.reservation_calls,
            );
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
        }

        // After: the hybrid merge, fed by the calling thread partitioning
        // every batch (as before) and by the payloads decoded on threads
        // of their own with every merge thread keeping its rows; under
        // the refusing budget, and under one that holds the merge.
        let mut runs = Vec::new();
        for budget_name in ["tight", "roomy"] {
            for variant in ["partitioned", "broadcast"] {
                for round in 1..=3 {
                    runs.push((budget_name, variant, round));
                }
            }
        }
        for (budget_name, variant, round) in runs {
            let budget = if budget_name == "tight" {
                budget
            } else {
                2u64 << 30
            };
            let pool = QueryMemoryPool::new("final-after", budget).unwrap();
            let spill = SpillManager::new(spill_root("after"), 8 << 30).unwrap();
            pool.shared_resource("kaveon.exec.hash-spill.v1", || Ok((spill.clone(), 16usize)))
                .unwrap();
            let started = std::time::Instant::now();
            let mut merged = if variant == "partitioned" {
                // The calling thread hash-partitions every batch to its
                // thread; each thread merges its part.
                let operator: ThreadOperator = Arc::new(move |source, pool, context| {
                    hybrid_final(source, pool, context.spill.clone())
                });
                ParallelPartials::partitioned(
                    ipc_sources_here(&payloads),
                    final_schema(),
                    vec!["group_keys".into()],
                    operator,
                    pool.clone(),
                    THREADS,
                )
                .unwrap()
            } else {
                // Three sources read on their own threads, every batch to
                // every merge thread, each keeping its own rows.
                let rendezvous = Rendezvous::new(THREADS);
                let operator: ThreadOperator = Arc::new(move |source, pool, context| {
                    hybrid_final_of(source, pool, context, &rendezvous)
                });
                ParallelPartials::broadcast(
                    ipc_sources_threads(&payloads),
                    final_schema(),
                    operator,
                    pool.clone(),
                    THREADS,
                )
                .unwrap()
            };
            let totals_all = totals(&mut merged).unwrap();
            let total = started.elapsed();
            assert_eq!(totals_all.rows, expected_groups);
            assert_eq!(totals_all.count, ROWS as u64);
            assert_eq!(totals_all.sum, expected_sum);
            let snapshot = spill.snapshot();
            let memory = pool.snapshot();
            println!(
                "after ({variant}, {budget_name}), round {round}: {total:.2?} wall, {} output \
                 batches; spill {} MiB written in {} runs, {} compactions, write {:.2?} read \
                 {:.2?}; memory peak {} MiB, {} reservation calls",
                totals_all.batches,
                snapshot.bytes_written >> 20,
                snapshot.runs_written,
                snapshot.compactions,
                std::time::Duration::from_micros(snapshot.write_us),
                std::time::Duration::from_micros(snapshot.read_us),
                memory.peak_bytes >> 20,
                memory.reservation_calls,
            );
            drop(merged);
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
        }
    }
}
