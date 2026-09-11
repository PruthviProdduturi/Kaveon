//! Disk-partitioned hash execution. Each partition must fit the query's memory
//! budget: pathological key skew fails closed rather than repartitioning forever.
//! This bounds retained run metadata and open readers, not upstream allocations
//! or Arrow IPC codec scratch space. Callers must account retained output batches.

use std::{collections::VecDeque, sync::Arc};

use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use kaveon_core::{
    BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, QueryMemoryPool, Result,
};

use crate::{
    aggregate::{
        AggExpr, HashAggregate, aggregate_output_types, grouped_aggregate_states_to_schema_batch,
    },
    exchange::HashPartitioner,
    join::{HashJoin, JoinType},
    spill::{SpillManager, SpillRun, SpillRunReader},
};

const MAX_PARTITIONS: usize = 256;
const MAX_RUNS_PER_PARTITION: usize = 16;
const MAX_ADAPTIVE_BATCHES: usize = 64;
// A grouped partial whose cardinality is too high is replayed through the
// partitioned spill path. Keep that speculative cardinality probe small: its
// hash table is discarded on rejection, so processing a large prefix repeats
// the most expensive work without changing the exact result.
const MAX_PARTIAL_PROBE_BATCHES: usize = 8;
// Once a bounded probe has been emitted, combine wider windows while they cut
// exchange rows by at least 4x. This avoids repartitioning repeated
// high-cardinality groups while rejecting genuinely unique, unbounded streams.
const MIN_STREAMING_PARTIAL_REDUCTION: usize = 4;

/// A bounded prefix can be replayed from memory without reopening its source.
struct BufferedPrefix {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    guards: Vec<Option<MemoryReservation>>,
    tail: Option<Box<dyn BatchOperator>>,
    complete: bool,
}

impl BufferedPrefix {
    fn collect(
        source: Box<dyn BatchOperator>,
        memory: &OperatorMemoryAccount,
        limit: u64,
    ) -> Result<Self> {
        Self::collect_with_batch_limit(source, memory, limit, MAX_ADAPTIVE_BATCHES)
    }

    fn collect_partial_distinct(
        source: Box<dyn BatchOperator>,
        memory: &OperatorMemoryAccount,
        limit: u64,
    ) -> Result<Self> {
        // A global DISTINCT state is usually much smaller than its input, but
        // its encoded partial still contains every admitted value. Let the
        // byte limit, rather than the generic adaptive batch-count limit,
        // bound how much input one worker combines before exchange.
        Self::collect_with_batch_limit(source, memory, limit, usize::MAX)
    }

    fn collect_with_batch_limit(
        mut source: Box<dyn BatchOperator>,
        memory: &OperatorMemoryAccount,
        limit: u64,
        batch_limit: usize,
    ) -> Result<Self> {
        let mut prefix = Self {
            schema: source.schema().clone(),
            batches: Vec::new(),
            guards: Vec::new(),
            tail: None,
            complete: false,
        };
        let mut bytes = 0u64;
        if limit == 0 {
            prefix.tail = Some(source);
            return Ok(prefix);
        }
        while prefix.batches.len() < batch_limit {
            memory.check_cancelled()?;
            let Some(batch) = source.next_batch()? else {
                prefix.complete = true;
                return Ok(prefix);
            };
            let cost = (batch.get_array_memory_size() as u64).saturating_add(1024);
            // The last batch is handed directly to the partitioner's normal
            // preflight, just like a batch returned by any upstream operator.
            if bytes.saturating_add(cost) > limit {
                prefix.batches.push(batch);
                prefix.guards.push(None);
                prefix.tail = Some(source);
                return Ok(prefix);
            }
            match memory.reserve(cost) {
                Ok(guard) => {
                    prefix.guards.push(Some(guard));
                    bytes += cost;
                    prefix.batches.push(batch);
                }
                Err(KaveonError::MemoryLimit(_)) => {
                    prefix.batches.push(batch);
                    prefix.guards.push(None);
                    prefix.tail = Some(source);
                    return Ok(prefix);
                }
                Err(error) => return Err(error),
            }
        }
        prefix.tail = Some(source);
        Ok(prefix)
    }

    fn trial(&self) -> Box<dyn BatchOperator> {
        Box::new(ReplayInput {
            schema: self.schema.clone(),
            batches: self.batches.clone().into(),
            tail: None,
            guards: VecDeque::new(),
            active_guard: None,
        })
    }

    fn replay(self) -> Box<dyn BatchOperator> {
        Box::new(ReplayInput {
            schema: self.schema,
            batches: self.batches.into(),
            tail: self.tail,
            guards: self.guards.into(),
            active_guard: None,
        })
    }

    fn take_tail(&mut self) -> Option<Box<dyn BatchOperator>> {
        self.tail.take()
    }

    fn row_count(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }
}

struct ReplayInput {
    schema: SchemaRef,
    batches: VecDeque<RecordBatch>,
    tail: Option<Box<dyn BatchOperator>>,
    guards: VecDeque<Option<MemoryReservation>>,
    active_guard: Option<MemoryReservation>,
}
impl BatchOperator for ReplayInput {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.active_guard = None;
        if let Some(batch) = self.batches.pop_front() {
            self.active_guard = self.guards.pop_front().flatten();
            return Ok(Some(batch));
        }
        self.tail
            .as_mut()
            .map_or(Ok(None), |tail| tail.next_batch())
    }
}

fn adaptive_limit(memory: &OperatorMemoryAccount) -> Result<u64> {
    let pool = memory.query().snapshot().limit_bytes;
    let default = (pool / 16).min(64 * 1024 * 1024);
    Ok(environment_integer("KAVEON_HASH_ADAPTIVE_BYTES", default)?.min(pool / 4))
}

/// Optional query-shared spill settings. All operators using the same pool use
/// one disk budget. This is not a process-wide or cluster-wide disk quota.
/// Absence of the root preserves memory-only execution.
pub fn spill_from_environment(memory: &QueryMemoryPool) -> Result<Option<(SpillManager, usize)>> {
    let Some(root) = std::env::var_os("KAVEON_HASH_SPILL_ROOT") else {
        return Ok(None);
    };
    if root.is_empty() {
        return Err(KaveonError::Execution(
            "KAVEON_HASH_SPILL_ROOT cannot be empty".into(),
        ));
    }
    let bytes = environment_integer("KAVEON_HASH_SPILL_BYTES", 10 * 1024 * 1024 * 1024)?;
    let count = usize::try_from(environment_integer("KAVEON_HASH_SPILL_PARTITIONS", 16)?)
        .map_err(|_| KaveonError::Execution("KAVEON_HASH_SPILL_PARTITIONS is too large".into()))?;
    validate_partitions(count)?;
    let resource = memory.shared_resource("kaveon.exec.hash-spill.v1", || {
        Ok((SpillManager::new(root, bytes)?, count))
    })?;
    Ok(Some((resource.0.clone(), resource.1)))
}

fn environment_integer(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| KaveonError::Execution(format!("{name} must be an unsigned integer"))),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(KaveonError::Execution(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

pub fn sort_operator(
    input: Box<dyn BatchOperator>,
    ordering: Vec<crate::sort::SortExpr>,
    memory: Option<OperatorMemoryAccount>,
) -> Result<Box<dyn BatchOperator>> {
    if let Some(memory) = memory {
        if let Some((spill, _)) = spill_from_environment(memory.query())? {
            return Ok(Box::new(crate::sort::SortOperator::new_with_spill(
                input, ordering, memory, spill,
            )?));
        }
        Ok(Box::new(
            crate::sort::SortOperator::new(input, ordering)?.with_memory(memory),
        ))
    } else {
        Ok(Box::new(crate::sort::SortOperator::new(input, ordering)?))
    }
}

pub fn top_n_operator(
    input: Box<dyn BatchOperator>,
    ordering: Vec<crate::sort::SortExpr>,
    limit: usize,
    memory: Option<OperatorMemoryAccount>,
) -> Result<Box<dyn BatchOperator>> {
    if let Some(memory) = memory {
        if let Some((spill, _)) = spill_from_environment(memory.query())? {
            return Ok(Box::new(crate::topn::TopNOperator::new_with_spill(
                input, ordering, limit, memory, spill,
            )?));
        }
        Ok(Box::new(
            crate::topn::TopNOperator::new(input, ordering, limit)?.with_memory(memory),
        ))
    } else {
        Ok(Box::new(crate::topn::TopNOperator::new(
            input, ordering, limit,
        )?))
    }
}

pub fn hash_aggregate(
    input: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: Option<OperatorMemoryAccount>,
) -> Result<Box<dyn BatchOperator>> {
    if let Some(memory) = memory {
        if let Some((spill, count)) = spill_from_environment(memory.query())? {
            return Ok(Box::new(PartitionedHashAggregate::new(
                input, group_by, aggregates, memory, spill, count,
            )?));
        }
        Ok(Box::new(HashAggregate::new_with_memory(
            input, group_by, aggregates, memory,
        )?))
    } else {
        Ok(Box::new(HashAggregate::new(input, group_by, aggregates)?))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn hash_join(
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    join_type: JoinType,
    keys: Vec<(String, String)>,
    left_qualifier: Option<&str>,
    right_qualifier: Option<&str>,
    memory: Option<OperatorMemoryAccount>,
) -> Result<Box<dyn BatchOperator>> {
    if let Some(memory) = memory {
        if let Some((spill, count)) = spill_from_environment(memory.query())? {
            return Ok(Box::new(PartitionedHashJoin::new(
                left,
                right,
                join_type,
                keys,
                left_qualifier,
                right_qualifier,
                memory,
                spill,
                count,
            )?));
        }
        Ok(Box::new(HashJoin::try_new_qualified_with_memory(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
            memory,
        )?))
    } else {
        Ok(Box::new(HashJoin::try_new_qualified(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
        )?))
    }
}

fn validate_partitions(count: usize) -> Result<()> {
    if !(1..=MAX_PARTITIONS).contains(&count) {
        return Err(KaveonError::Execution(format!(
            "spill partition count must be between 1 and {MAX_PARTITIONS}"
        )));
    }
    Ok(())
}

/// Reads runs sequentially, keeping only one file open. Dropping the source
/// removes all remaining runs, including after operator failure/cancellation.
struct RunSource {
    schema: SchemaRef,
    runs: VecDeque<SpillRun>,
    reader: Option<SpillRunReader>,
}

impl RunSource {
    fn new(schema: SchemaRef, runs: Vec<SpillRun>) -> Self {
        Self {
            schema,
            runs: runs.into(),
            reader: None,
        }
    }
}

impl BatchOperator for RunSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(reader) = &mut self.reader {
                if let Some(batch) = reader.next() {
                    return batch.map(Some);
                }
                // Close the handle before deleting a run (required on Windows).
                self.reader = None;
                self.runs.pop_front();
            }
            let Some(run) = self.runs.front() else {
                return Ok(None);
            };
            self.reader = Some(run.reader()?);
        }
    }
}

impl Drop for RunSource {
    fn drop(&mut self) {
        self.reader = None;
    }
}

fn partition_input(
    input: &mut dyn BatchOperator,
    keys: &[String],
    count: usize,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
    input_reserved: bool,
) -> Result<Vec<Vec<SpillRun>>> {
    let schema = Arc::clone(input.schema());
    let partitioner = if keys.is_empty() {
        None
    } else {
        Some(HashPartitioner::try_new(&schema, keys, count)?)
    };
    let mut partitions: Vec<Vec<SpillRun>> = (0..count).map(|_| Vec::new()).collect();
    let mut buffered: Vec<Vec<RecordBatch>> = (0..count).map(|_| Vec::new()).collect();
    let mut buffered_memory = Vec::new();
    let mut buffered_bytes = 0_u64;
    // Batch several upstream partitions into each Arrow stream. The old path
    // created one tiny run per non-empty partition and upstream batch, then
    // paid to compact those files repeatedly. Retaining the existing
    // conservative preflight reservations keeps this buffer query-bounded.
    let flush_bytes = adaptive_limit(memory)?.max(1);
    while let Some(batch) = input.next_batch()? {
        if batch.num_rows() == 0 {
            continue;
        }
        // Conservatively cover input, copied partitions, row encoding, indices,
        // and per-partition array headers before constructing them. An upstream
        // batch larger than the budget is rejected, never silently unaccounted.
        let input_bytes = if input_reserved {
            batch
                .columns()
                .iter()
                .map(|column| {
                    column
                        .to_data()
                        .get_slice_memory_size()
                        .map(|bytes| bytes as u64)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .sum()
        } else {
            batch.get_array_memory_size() as u64
        };
        let bytes = input_bytes
            .checked_mul(if input_reserved { 3 } else { 4 })
            .and_then(|n| n.checked_add((batch.num_rows() as u64).saturating_mul(32)))
            .and_then(|n| {
                n.checked_add(
                    (count as u64)
                        .saturating_mul(schema.fields().len() as u64)
                        .saturating_mul(256),
                )
            })
            .ok_or_else(|| {
                KaveonError::Execution("spill partition memory estimate overflow".into())
            })?;
        let reservation = memory.reserve(bytes)?;
        let batches = match &partitioner {
            Some(partitioner) => partitioner.partition(&batch)?,
            None => vec![batch],
        };
        for (index, batch) in batches.into_iter().enumerate() {
            if batch.num_rows() == 0 {
                continue;
            }
            buffered[index].push(batch);
        }
        buffered_bytes = buffered_bytes.saturating_add(reservation.bytes());
        buffered_memory.push(reservation);
        if buffered_bytes >= flush_bytes {
            flush_partition_buffers(
                &mut buffered,
                &mut buffered_memory,
                &mut buffered_bytes,
                &mut partitions,
                &schema,
                memory,
                spill,
            )?;
        }
    }
    flush_partition_buffers(
        &mut buffered,
        &mut buffered_memory,
        &mut buffered_bytes,
        &mut partitions,
        &schema,
        memory,
        spill,
    )?;
    Ok(partitions)
}

#[allow(clippy::too_many_arguments)]
fn flush_partition_buffers(
    buffered: &mut [Vec<RecordBatch>],
    buffered_memory: &mut Vec<MemoryReservation>,
    buffered_bytes: &mut u64,
    partitions: &mut [Vec<SpillRun>],
    schema: &SchemaRef,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
) -> Result<()> {
    for (batches, runs) in buffered.iter_mut().zip(partitions.iter_mut()) {
        if batches.is_empty() {
            continue;
        }
        runs.push(spill.write_run(schema, batches)?);
        batches.clear();
        compact_spill_runs(runs, schema, memory, spill)?;
    }
    buffered_memory.clear();
    *buffered_bytes = 0;
    Ok(())
}

/// Keeps reader fan-in bounded without repeatedly rewriting a partition's full
/// history. Merging the two smallest runs gives leveled compaction: old, large
/// runs are only rewritten after newer runs have grown to a comparable size.
fn compact_spill_runs(
    runs: &mut Vec<SpillRun>,
    schema: &SchemaRef,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
) -> Result<()> {
    while runs.len() >= MAX_RUNS_PER_PARTITION {
        let mut by_size = runs
            .iter()
            .enumerate()
            .map(|(index, run)| (run.bytes(), index))
            .collect::<Vec<_>>();
        by_size.sort_unstable();
        let mut selected = [by_size[0].1, by_size[1].1];
        selected.sort_unstable();
        let right = runs.remove(selected[1]);
        let left = runs.remove(selected[0]);
        spill.record_compaction(left.bytes().saturating_add(right.bytes()));
        let mut source = RunSource::new(Arc::clone(schema), vec![left, right]);
        let mut batch_memory = None;
        let batches = std::iter::from_fn(|| {
            batch_memory = None;
            source.next_batch().transpose().map(|batch| {
                let batch = batch?;
                batch_memory =
                    Some(memory.reserve((batch.get_array_memory_size() as u64).saturating_mul(2))?);
                Ok(batch)
            })
        });
        runs.push(spill.write_run_stream(schema, batches)?);
    }
    Ok(())
}

/// Spools canonical state batches into independently consumable partitions.
/// Empty input retains its schema through one empty source. Consumers must
/// process one source at a time and drop remaining sources on failure.
pub fn partition_sources(
    mut input: Box<dyn BatchOperator>,
    keys: &[String],
    partition_count: usize,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
) -> Result<VecDeque<Box<dyn BatchOperator>>> {
    validate_partitions(partition_count)?;
    let schema = Arc::clone(input.schema());
    let count = if keys.is_empty() { 1 } else { partition_count };
    let partitions = partition_input(input.as_mut(), keys, count, memory, spill, false)?;
    let mut sources: VecDeque<Box<dyn BatchOperator>> = partitions
        .into_iter()
        .filter(|runs| !runs.is_empty())
        .map(|runs| Box::new(RunSource::new(Arc::clone(&schema), runs)) as Box<dyn BatchOperator>)
        .collect();
    if sources.is_empty() {
        sources.push_back(Box::new(RunSource::new(schema, Vec::new())));
    }
    Ok(sources)
}

pub struct PartitionedHashAggregate {
    input: Option<Box<dyn BatchOperator>>,
    input_schema: SchemaRef,
    schema: SchemaRef,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    memory: OperatorMemoryAccount,
    spill: SpillManager,
    count: usize,
    partitions: VecDeque<Vec<SpillRun>>,
    failed: bool,
    partial: bool,
    input_reserved: bool,
    output_memory: Option<MemoryReservation>,
    adaptive_bytes: Option<u64>,
    streaming_partial: bool,
    partial_probe_complete: bool,
}

impl PartitionedHashAggregate {
    pub fn new(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
        partition_count: usize,
    ) -> Result<Self> {
        validate_partitions(partition_count)?;
        let input_schema = Arc::clone(source.schema());
        let probe = HashAggregate::new(
            Box::new(RunSource::new(Arc::clone(&input_schema), Vec::new())),
            group_by.clone(),
            aggregates.clone(),
        )?;
        let count = if group_by.is_empty() {
            1
        } else {
            partition_count
        };
        Ok(Self {
            input: Some(source),
            input_schema,
            schema: Arc::clone(probe.schema()),
            group_by,
            aggregates,
            memory,
            spill,
            count,
            partitions: VecDeque::new(),
            failed: false,
            partial: false,
            input_reserved: false,
            output_memory: None,
            adaptive_bytes: None,
            streaming_partial: true,
            partial_probe_complete: false,
        })
    }

    /// The source must retain a reservation for each emitted input batch until
    /// the next source call. Partition copies and execution state remain charged.
    pub fn with_reserved_input(mut self) -> Self {
        self.input_reserved = true;
        self
    }

    pub fn new_partial(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
        partition_count: usize,
    ) -> Result<Self> {
        let mut operator = Self::new(source, group_by, aggregates, memory, spill, partition_count)?;
        operator.partial = true;
        operator.schema = grouped_aggregate_states_to_schema_batch(
            &[],
            &operator.group_types()?,
            &aggregate_output_types(&operator.aggregates, &operator.input_schema)?,
        )?
        .schema();
        Ok(operator)
    }

    fn group_types(&self) -> Result<Vec<arrow::datatypes::DataType>> {
        self.group_by
            .iter()
            .map(|name| {
                self.input_schema
                    .field_with_name(name)
                    .map(|field| field.data_type().clone())
                    .map_err(KaveonError::from)
            })
            .collect()
    }

    fn aggregate_batch(
        &self,
        source: Box<dyn BatchOperator>,
        reserved: bool,
    ) -> Result<(Option<RecordBatch>, Option<MemoryReservation>)> {
        let mut operator = HashAggregate::new_with_memory(
            source,
            self.group_by.clone(),
            self.aggregates.clone(),
            self.memory.clone(),
        )?;
        if reserved {
            operator = operator.with_reserved_input();
        }
        let batch = if self.partial {
            let (states, state_memory) = operator.into_grouped_states_with_reservations()?;
            let bytes = state_memory
                .iter()
                .map(|reservation| reservation.bytes())
                .sum::<u64>()
                .saturating_mul(4)
                .saturating_add((states.len() as u64).saturating_mul(4096));
            let _encoding_memory = self.memory.reserve(bytes)?;
            Some(grouped_aggregate_states_to_schema_batch(
                &states,
                &self.group_types()?,
                &aggregate_output_types(&self.aggregates, &self.input_schema)?,
            )?)
        } else {
            operator.next_batch()?
        };
        let guard = batch
            .as_ref()
            .map(|batch| self.memory.reserve(batch.get_array_memory_size() as u64))
            .transpose()?;
        Ok((batch, guard))
    }

    fn execute_next(&mut self) -> Result<Option<RecordBatch>> {
        self.output_memory = None;
        if self.partial && self.streaming_partial && self.partitions.is_empty() {
            let Some(mut input) = self.input.take() else {
                return Ok(None);
            };
            let global_distinct = self.group_by.is_empty()
                && self.aggregates.iter().any(|aggregate| aggregate.distinct);
            if global_distinct {
                let mut prefix = BufferedPrefix::collect_partial_distinct(
                    input,
                    &self.memory,
                    self.adaptive_bytes.unwrap_or(adaptive_limit(&self.memory)?),
                )?;
                match self.aggregate_batch(prefix.trial(), true) {
                    Ok((batch, guard)) => {
                        self.input = prefix.take_tail();
                        self.output_memory = guard;
                        return Ok(batch);
                    }
                    Err(KaveonError::MemoryLimit(_)) => {
                        // Preserve the one-batch streaming fallback when the
                        // encoded DISTINCT state cannot fit beside the prefix.
                        self.memory.check_cancelled()?;
                        input = prefix.replay();
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut prefix = BufferedPrefix::collect_with_batch_limit(
                input,
                &self.memory,
                self.adaptive_bytes.unwrap_or(adaptive_limit(&self.memory)?),
                if global_distinct {
                    1
                } else if self.partial_probe_complete {
                    MAX_ADAPTIVE_BATCHES
                } else {
                    MAX_PARTIAL_PROBE_BATCHES
                },
            )?;
            let input_rows = prefix.row_count();
            if input_rows != 0 {
                match self.aggregate_batch(prefix.trial(), true) {
                    Ok((batch, guard))
                        if batch.as_ref().is_some_and(|batch| {
                            !self.partial_probe_complete
                                || prefix.complete
                                || batch
                                    .num_rows()
                                    .saturating_mul(MIN_STREAMING_PARTIAL_REDUCTION)
                                    <= input_rows
                        }) =>
                    {
                        // Partial states are mergeable across windows. Emit the
                        // small initial probe, then retain the streaming path only
                        // for useful reduction or a complete, successful tail.
                        self.input = prefix.take_tail();
                        self.output_memory = guard;
                        self.partial_probe_complete = true;
                        return Ok(batch);
                    }
                    Ok(_) | Err(KaveonError::MemoryLimit(_)) => {
                        self.memory.check_cancelled()?;
                        self.streaming_partial = false;
                        self.input = Some(prefix.replay());
                    }
                    Err(error) => return Err(error),
                }
            } else {
                let (batch, guard) = self.aggregate_batch(
                    Box::new(RunSource::new(Arc::clone(&self.input_schema), Vec::new())),
                    false,
                )?;
                if batch.as_ref().is_some_and(|batch| batch.num_rows() > 0) {
                    self.output_memory = guard;
                    return Ok(batch);
                }
                return Ok(None);
            }
        }
        if let Some(input) = self.input.take() {
            let prefix = BufferedPrefix::collect(
                input,
                &self.memory,
                self.adaptive_bytes.unwrap_or(adaptive_limit(&self.memory)?),
            )?;
            if prefix.complete {
                match self.aggregate_batch(prefix.trial(), true) {
                    Ok((batch, guard)) => {
                        self.output_memory = guard;
                        return Ok(batch);
                    }
                    Err(KaveonError::MemoryLimit(_)) => {
                        self.memory.check_cancelled()?;
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut input = prefix.replay();
            self.partitions = partition_input(
                input.as_mut(),
                &self.group_by,
                self.count,
                &self.memory,
                &self.spill,
                self.input_reserved,
            )?
            .into();
        }
        while let Some(runs) = self.partitions.pop_front() {
            if runs.is_empty() && !self.group_by.is_empty() {
                continue;
            }
            let (batch, guard) = self.aggregate_batch(
                Box::new(RunSource::new(self.input_schema.clone(), runs)),
                false,
            )?;
            self.output_memory = guard;
            return Ok(batch);
        }
        Ok(None)
    }
}

impl BatchOperator for PartitionedHashAggregate {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.failed {
            return Ok(None);
        }
        let result = self.execute_next();
        if result.is_err() {
            self.failed = true;
            self.partitions.clear();
            self.input = None;
        }
        result
    }
}

pub struct PartitionedHashJoin {
    left: Option<Box<dyn BatchOperator>>,
    right: Option<Box<dyn BatchOperator>>,
    left_schema: SchemaRef,
    right_schema: SchemaRef,
    schema: SchemaRef,
    join_type: JoinType,
    keys: Vec<(String, String)>,
    left_qualifier: Option<String>,
    right_qualifier: Option<String>,
    memory: OperatorMemoryAccount,
    spill: SpillManager,
    count: usize,
    partitions: VecDeque<(Vec<SpillRun>, Vec<SpillRun>)>,
    failed: bool,
    active: Option<HashJoin>,
    adaptive_bytes: Option<u64>,
}

impl PartitionedHashJoin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
        partition_count: usize,
    ) -> Result<Self> {
        validate_partitions(partition_count)?;
        let left_schema = Arc::clone(left.schema());
        let right_schema = Arc::clone(right.schema());
        let probe = HashJoin::try_new_qualified(
            Box::new(RunSource::new(Arc::clone(&left_schema), Vec::new())),
            Box::new(RunSource::new(Arc::clone(&right_schema), Vec::new())),
            join_type,
            keys.clone(),
            left_qualifier,
            right_qualifier,
        )?;
        // Use exact schema names after the join's normal ambiguity checks.
        let keys = keys
            .into_iter()
            .map(|(left, right)| {
                Ok((
                    resolve_key(&left_schema, &left)?,
                    resolve_key(&right_schema, &right)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            left: Some(left),
            right: Some(right),
            left_schema,
            right_schema,
            schema: Arc::clone(probe.schema()),
            join_type,
            keys,
            left_qualifier: left_qualifier.map(str::to_owned),
            right_qualifier: right_qualifier.map(str::to_owned),
            memory,
            spill,
            count: if join_type == JoinType::Cross {
                1
            } else {
                partition_count
            },
            partitions: VecDeque::new(),
            failed: false,
            active: None,
            adaptive_bytes: None,
        })
    }

    fn execute_next(&mut self) -> Result<Option<RecordBatch>> {
        if let Some(active) = &mut self.active {
            if let Some(batch) = active.next_batch()? {
                return Ok(Some(batch));
            }
            self.active = None;
        }
        if let Some(left) = self.left.take() {
            let limit = self.adaptive_bytes.unwrap_or(adaptive_limit(&self.memory)?) / 2;
            let right = self
                .right
                .take()
                .expect("right input exists before partitioning");
            let right_prefix = BufferedPrefix::collect(right, &self.memory, limit)?;
            // HashJoin retains only its build side and streams the probe. A
            // large probe therefore must not force disk partitioning merely
            // because it exceeds the small adaptive prefix. This was creating
            // hundreds of Arrow runs for the 5M-row grouped join even though
            // the 100k-row build side fit comfortably in memory.
            if right_prefix.complete && streaming_build_fits(&right_prefix, &self.memory)? {
                let mut operator = HashJoin::try_new_qualified_with_memory(
                    left,
                    right_prefix.replay(),
                    self.join_type,
                    self.keys.clone(),
                    self.left_qualifier.as_deref(),
                    self.right_qualifier.as_deref(),
                    self.memory.clone(),
                )?;
                let batch = operator.next_batch()?;
                self.active = Some(operator);
                return Ok(batch);
            }
            let mut left = left;
            let mut right = right_prefix.replay();
            let left_keys = self
                .keys
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let right_keys = self
                .keys
                .iter()
                .map(|(_, key)| key.clone())
                .collect::<Vec<_>>();
            let left_runs = partition_input(
                left.as_mut(),
                &left_keys,
                self.count,
                &self.memory,
                &self.spill,
                false,
            )?;
            let right_runs = partition_input(
                right.as_mut(),
                &right_keys,
                self.count,
                &self.memory,
                &self.spill,
                false,
            )?;
            self.partitions = left_runs.into_iter().zip(right_runs).collect();
        }
        while let Some((left, right)) = self.partitions.pop_front() {
            if left.is_empty() && right.is_empty() {
                continue;
            }
            let mut operator = HashJoin::try_new_qualified_with_memory(
                Box::new(RunSource::new(Arc::clone(&self.left_schema), left)),
                Box::new(RunSource::new(Arc::clone(&self.right_schema), right)),
                self.join_type,
                self.keys.clone(),
                self.left_qualifier.as_deref(),
                self.right_qualifier.as_deref(),
                self.memory.clone(),
            )?;
            let batch = operator.next_batch()?;
            self.active = Some(operator);
            if batch.is_none() {
                self.active = None;
                continue;
            }
            return Ok(batch);
        }
        Ok(None)
    }
}

fn streaming_build_fits(prefix: &BufferedPrefix, memory: &OperatorMemoryAccount) -> Result<bool> {
    let (bytes, rows) =
        prefix
            .batches
            .iter()
            .try_fold((0_u64, 0_u64), |(bytes, rows), batch| {
                Ok::<_, KaveonError>((
                    bytes
                        .checked_add(batch.get_array_memory_size() as u64)
                        .ok_or_else(|| KaveonError::Execution("join build size overflow".into()))?,
                    rows.checked_add(batch.num_rows() as u64).ok_or_else(|| {
                        KaveonError::Execution("join build row count overflow".into())
                    })?,
                ))
            })?;
    // HashJoin reserves a concatenated build copy plus key/index storage. Keep
    // half of currently available query memory for probe/output operators.
    let required = bytes
        .checked_mul(2)
        .and_then(|value| value.checked_add(rows.saturating_mul(64)))
        .ok_or_else(|| KaveonError::Execution("join build estimate overflow".into()))?;
    let snapshot = memory.query().snapshot();
    Ok(required <= snapshot.limit_bytes.saturating_sub(snapshot.current_bytes) / 2)
}

impl BatchOperator for PartitionedHashJoin {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.failed {
            return Ok(None);
        }
        let result = self.execute_next();
        if result.is_err() {
            self.failed = true;
            self.active = None;
            self.partitions.clear();
            self.left = None;
            self.right = None;
        }
        result
    }
}

fn resolve_key(schema: &SchemaRef, name: &str) -> Result<String> {
    if schema.index_of(name).is_ok() {
        return Ok(name.to_owned());
    }
    let suffix = name.rsplit('.').next().unwrap_or(name);
    let names = schema
        .fields()
        .iter()
        .filter(|field| field.name().rsplit('.').next() == Some(suffix))
        .collect::<Vec<_>>();
    match names.as_slice() {
        [field] => Ok(field.name().clone()),
        _ => Err(KaveonError::Execution(format!(
            "spill join key '{name}' is missing or ambiguous"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{
        AggFunc, aggregate_metrics, finalize_grouped_aggregate_states,
        grouped_aggregate_states_from_batches, merge_grouped_aggregate_states,
    };
    use arrow::{
        array::{Array, Int64Array, UInt64Array},
        datatypes::{DataType, Field, Schema},
    };
    use kaveon_core::QueryMemoryPool;

    #[test]
    fn projected_repeat_uses_string_schema_and_query_expansion_budget() {
        let expression = |count| kaveon_core::Expr::Function {
            name: "REPEAT".into(),
            args: vec![
                kaveon_core::Expr::Literal(kaveon_core::predicate::ScalarValue::Utf8("ab".into())),
                kaveon_core::Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(count)),
            ],
        };
        let pool = QueryMemoryPool::new("project-repeat", 4096).unwrap();
        let mut valid =
            crate::project::ProjectOperator::new(input(vec![Some(1)], 1), vec![expression(3)])
                .unwrap()
                .with_memory(pool.operator("valid").unwrap());
        let batch = valid.next_batch().unwrap().unwrap();
        assert_eq!(batch.schema().field(0).data_type(), &DataType::Utf8);
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .unwrap()
                .value(0),
            "ababab"
        );
        let mut excessive =
            crate::project::ProjectOperator::new(input(vec![Some(1)], 1), vec![expression(10000)])
                .unwrap()
                .with_memory(pool.operator("excessive").unwrap());
        assert!(excessive.next_batch().is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn running_window_checks_cancellation_inside_frame_evaluation() {
        let pool = QueryMemoryPool::new("window-cancel", 8 * 1024 * 1024).unwrap();
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = polls.clone();
        pool.set_cancellation_probe(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 8
        })
        .unwrap();
        let expression = kaveon_core::Expr::WindowFunction {
            name: "SUM".into(),
            args: vec![kaveon_core::Expr::Column("id".into())],
            partition_by: vec![],
            order_by: vec![(kaveon_core::Expr::Column("id".into()), true)],
            frame: None,
        };
        let mut window = crate::window::WindowOperator::new(
            input((0..1000).map(Some).collect(), 1000),
            vec![expression],
        )
        .unwrap()
        .with_memory(pool.operator("window").unwrap());
        let error = window.next_batch().unwrap_err().to_string();
        assert!(error.contains("query canceled"));
        assert!(polls.load(std::sync::atomic::Ordering::Relaxed) >= 9);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn window_distinct_and_semi_join_fail_at_budget_and_release_on_drop() {
        let values = (0..1000).map(Some).collect::<Vec<_>>();
        let pool = QueryMemoryPool::new("bounded-operators", 4096).unwrap();
        let mut distinct = crate::distinct::DistinctOperator::new(input(values.clone(), 10))
            .with_memory(pool.operator("distinct").unwrap());
        while matches!(distinct.next_batch(), Ok(Some(_))) {}
        assert!(pool.snapshot().current_bytes > 0);
        drop(distinct);
        assert_eq!(pool.snapshot().current_bytes, 0);
        let mut semi = crate::semijoin::SemiJoinOperator::new(
            input(vec![Some(1)], 1),
            input(values.clone(), 10),
            kaveon_core::Expr::Column("id".into()),
            kaveon_core::Expr::Column("id".into()),
            false,
        )
        .unwrap()
        .with_memory(pool.operator("semi").unwrap());
        assert!(semi.next_batch().is_err());
        drop(semi);
        assert_eq!(pool.snapshot().current_bytes, 0);
        let expression = kaveon_core::Expr::WindowFunction {
            name: "ROW_NUMBER".into(),
            args: vec![],
            partition_by: vec![],
            order_by: vec![],
            frame: None,
        };
        let mut window = crate::window::WindowOperator::new(input(values, 10), vec![expression])
            .unwrap()
            .with_memory(pool.operator("window").unwrap());
        assert!(window.next_batch().is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= 4096);
    }

    #[test]
    fn set_operations_deduplicate_across_batches_and_account_build_state() {
        for (mode, expected) in [
            (crate::setop::SetOpMode::Intersect, vec![None, Some(1)]),
            (crate::setop::SetOpMode::Except, vec![Some(2)]),
        ] {
            let pool = QueryMemoryPool::new("set-semantics", 16 * 1024).unwrap();
            let mut operator = crate::setop::SetOpOperator::new(
                input(vec![Some(1), Some(1), None, None, Some(2), Some(2)], 2),
                input(vec![Some(1), None, Some(1)], 2),
                mode,
            )
            .with_memory(pool.operator("set").unwrap());
            let mut values = Vec::new();
            while let Some(batch) = operator.next_batch().unwrap() {
                values.extend(
                    batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .unwrap()
                        .iter(),
                );
            }
            values.sort();
            assert_eq!(values, expected);
            assert_eq!(pool.snapshot().current_bytes, 0);
        }
        let pool = QueryMemoryPool::new("set-budget", 4096).unwrap();
        let mut operator = crate::setop::SetOpOperator::new(
            input(vec![], 1),
            input((0..1000).map(Some).collect(), 10),
            crate::setop::SetOpMode::Except,
        )
        .with_memory(pool.operator("set").unwrap());
        assert!(operator.next_batch().is_err());
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn sort_and_topn_never_bypass_an_oversized_input_reservation() {
        let pool = QueryMemoryPool::new("sort-oversize", 512).unwrap();
        let spill = spill();
        let ordering = vec![crate::sort::SortExpr::new(
            kaveon_core::Expr::Column("id".into()),
            true,
        )];
        let mut sort = crate::sort::SortOperator::new_with_spill(
            input(vec![Some(1), Some(2)], 2),
            ordering.clone(),
            pool.operator("sort").unwrap(),
            spill.clone(),
        )
        .unwrap();
        assert!(sort.next_batch().is_err());
        drop(sort);
        let mut topn = crate::topn::TopNOperator::new_with_spill(
            input(vec![Some(1), Some(2)], 2),
            ordering,
            1,
            pool.operator("topn").unwrap(),
            spill.clone(),
        )
        .unwrap();
        assert!(topn.next_batch().is_err());
        drop(topn);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }
    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    fn input(values: Vec<Option<i64>>, chunk: usize) -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        let batches = values
            .chunks(chunk)
            .map(|rows| {
                RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(Int64Array::from(rows.to_vec()))],
                )
                .unwrap()
            })
            .collect();
        Box::new(Input { schema, batches })
    }
    fn spill() -> SpillManager {
        SpillManager::new(
            std::env::temp_dir().join("kaveon-partition-tests"),
            64 * 1024 * 1024,
        )
        .unwrap()
    }

    #[test]
    fn spill_run_compaction_is_size_tiered() {
        let pool = QueryMemoryPool::new("tiered-compaction", 4 * 1024 * 1024).unwrap();
        let account = pool.operator("compact").unwrap();
        let disk = spill();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        let mut runs = Vec::new();
        for start in (0..16_384_i64).step_by(128) {
            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(Int64Array::from_iter_values(start..start + 128))],
            )
            .unwrap();
            runs.push(disk.write_run(&schema, &[batch]).unwrap());
            compact_spill_runs(&mut runs, &schema, &account, &disk).unwrap();
            assert!(runs.len() < MAX_RUNS_PER_PARTITION);
        }

        // Full-history compaction leaves one dominant run and rewrites it every
        // time the cap is reached. Size-tiered compaction retains balanced levels.
        let total_bytes = runs.iter().map(SpillRun::bytes).sum::<u64>();
        let largest = runs.iter().map(SpillRun::bytes).max().unwrap();
        assert!(largest < total_bytes / 2);
        let snapshot = disk.snapshot();
        assert!(snapshot.compactions > 0);
        assert!(snapshot.compaction_input_bytes > 0);
        assert!(snapshot.bytes_written > snapshot.current_bytes);
        assert!(snapshot.runs_written > runs.len() as u64);

        let mut source = RunSource::new(Arc::clone(&schema), runs);
        let mut rows = 0;
        while let Some(batch) = source.next_batch().unwrap() {
            rows += batch.num_rows();
        }
        assert_eq!(rows, 16_384);
        assert_eq!(disk.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn partition_spill_coalesces_tiny_upstream_batches_with_bounded_memory() {
        let pool = QueryMemoryPool::new("coalesced-partitions", 4 * 1024 * 1024).unwrap();
        let account = pool.operator("partition").unwrap();
        let disk = spill();
        let values = (0..1_024).map(Some).collect::<Vec<_>>();
        let mut sources =
            partition_sources(input(values, 1), &["id".into()], 16, &account, &disk).unwrap();

        let snapshot = disk.snapshot();
        assert!(snapshot.runs_written < 512, "{snapshot:?}");
        assert!(snapshot.compactions < 64, "{snapshot:?}");
        assert_eq!(pool.snapshot().current_bytes, 0);

        let mut rows = 0;
        while let Some(mut source) = sources.pop_front() {
            while let Some(batch) = source.next_batch().unwrap() {
                rows += batch.num_rows();
            }
        }
        assert_eq!(rows, 1_024);
        assert_eq!(disk.snapshot().current_bytes, 0);
    }

    #[test]
    fn partial_aggregate_combines_low_cardinality_batches_without_spill() {
        let pool = QueryMemoryPool::new("stream-partial", 32 * 1024 * 1024).unwrap();
        let disk = spill();
        let values = (0..1_700_000).map(|value| Some(value % 17)).collect();
        let mut aggregate = PartitionedHashAggregate::new_partial(
            input(values, 8_192),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("partial").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        let mut batches = Vec::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            batches.push(batch);
        }
        // 208 source batches are combined in byte- and count-bounded windows,
        // avoiding one encoded partial-state batch per source batch.
        assert!(batches.len() <= 8, "{} output batches", batches.len());
        let states = grouped_aggregate_states_from_batches(&batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        let finalized = finalize_grouped_aggregate_states(&merged).unwrap();
        assert_eq!(finalized.len(), 17);
        assert_eq!(
            finalized
                .iter()
                .map(|group| match group.values[0] {
                    crate::aggregate::FinalAggregateValue::Count(value) => value,
                    _ => panic!("expected count state"),
                })
                .sum::<u64>(),
            1_700_000
        );
        assert_eq!(disk.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn partial_global_distinct_combines_across_worker_batches() {
        let pool = QueryMemoryPool::new("combined-global-distinct", 128 * 1024 * 1024).unwrap();
        let disk = spill();
        let mut aggregate = PartitionedHashAggregate::new_partial(
            input((0..400_000).map(|value| Some(value % 1_000)).collect(), 128),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "id").distinct()],
            pool.operator("partial").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        let mut batches = Vec::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            batches.push(batch);
        }

        // The generic streaming path would emit one state for each of the
        // 3,125 source batches. The byte-bounded DISTINCT window can combine
        // this complete worker input into one exact state.
        assert_eq!(batches.len(), 1);
        let states = grouped_aggregate_states_from_batches(&batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        let finalized = finalize_grouped_aggregate_states(&merged).unwrap();
        assert_eq!(finalized.len(), 1);
        assert!(matches!(
            finalized[0].values[0],
            crate::aggregate::FinalAggregateValue::Count(1_000)
        ));
        assert_eq!(disk.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn partial_global_distinct_releases_memory_when_cancelled_during_combine() {
        let pool = QueryMemoryPool::new("cancel-global-distinct", 8 * 1024 * 1024).unwrap();
        pool.set_cancellation_probe(|| true).unwrap();
        let mut aggregate = PartitionedHashAggregate::new_partial(
            input((0..10_000).map(Some).collect(), 128),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "id").distinct()],
            pool.operator("partial").unwrap(),
            spill(),
            16,
        )
        .unwrap();

        assert!(
            aggregate
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("query canceled")
        );
        drop(aggregate);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn partial_global_distinct_replays_prefix_when_combine_exceeds_memory() {
        let pool = QueryMemoryPool::new("bounded-global-distinct", 1024 * 1024).unwrap();
        let mut aggregate = PartitionedHashAggregate::new_partial(
            input((0..10_000).map(Some).collect(), 128),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "id").distinct()],
            pool.operator("partial").unwrap(),
            spill(),
            16,
        )
        .unwrap();
        // Force the speculative prefix to consume the available budget. Its
        // aggregate cannot fit beside those retained batches, so execution
        // must replay them through the original bounded one-batch path.
        aggregate.adaptive_bytes = Some(256 * 1024);
        let mut batches = Vec::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            batches.push(batch);
        }

        assert!(batches.len() > 1);
        let states = grouped_aggregate_states_from_batches(&batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        let finalized = finalize_grouped_aggregate_states(&merged).unwrap();
        assert!(matches!(
            finalized[0].values[0],
            crate::aggregate::FinalAggregateValue::Count(10_000)
        ));
        assert!(pool.snapshot().peak_bytes <= 1024 * 1024);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn streaming_partial_aggregate_preserves_empty_and_memory_bound_semantics() {
        let global_pool = QueryMemoryPool::new("empty-partial", 64 * 1024).unwrap();
        let mut global = PartitionedHashAggregate::new_partial(
            input(vec![], 10),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "*")],
            global_pool.operator("global").unwrap(),
            spill(),
            16,
        )
        .unwrap();
        let batch = global.next_batch().unwrap().unwrap();
        let states = grouped_aggregate_states_from_batches(&[batch]).unwrap();
        let finalized = finalize_grouped_aggregate_states(&states).unwrap();
        assert_eq!(finalized.len(), 1);
        assert!(matches!(
            finalized[0].values[0],
            crate::aggregate::FinalAggregateValue::Count(0)
        ));
        assert!(global.next_batch().unwrap().is_none());
        assert_eq!(global_pool.snapshot().current_bytes, 0);

        let grouped_pool = QueryMemoryPool::new("empty-grouped-partial", 64 * 1024).unwrap();
        let mut grouped = PartitionedHashAggregate::new_partial(
            input(vec![], 10),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            grouped_pool.operator("grouped").unwrap(),
            spill(),
            16,
        )
        .unwrap();
        assert!(grouped.next_batch().unwrap().is_none());
        assert_eq!(grouped_pool.snapshot().current_bytes, 0);

        let bounded_pool = QueryMemoryPool::new("bounded-partial", 512 * 1024 * 1024).unwrap();
        let bounded_spill = spill();
        let mut high_cardinality = PartitionedHashAggregate::new_partial(
            input((0..100_000).map(Some).collect(), 8_192),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            bounded_pool.operator("high-cardinality").unwrap(),
            bounded_spill.clone(),
            16,
        )
        .unwrap();
        let mut high_cardinality_batches = Vec::new();
        while let Some(batch) = high_cardinality.next_batch().unwrap() {
            high_cardinality_batches.push(batch);
        }
        let states = grouped_aggregate_states_from_batches(&high_cardinality_batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        assert_eq!(merged.len(), 100_000);
        // A successful bounded probe can be emitted even when its rows are
        // unique; the complete tail is also exact and needs no disk replay.
        assert_eq!(
            aggregate_metrics(&bounded_pool)
                .unwrap()
                .snapshot()
                .input_rows,
            100_000
        );
        assert_eq!(bounded_spill.snapshot().peak_bytes, 0);
        assert_eq!(bounded_pool.snapshot().current_bytes, 0);

        // A continuing unique stream fails the wider-window reduction gate and
        // replays only its unread tail through the bounded spill path.
        let unique_pool = QueryMemoryPool::new("unique-partial", 64 * 1024 * 1024).unwrap();
        let unique_spill = spill();
        let mut unique = PartitionedHashAggregate::new_partial(
            input((0..10_000).map(Some).collect(), 128),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            unique_pool.operator("unique").unwrap(),
            unique_spill.clone(),
            16,
        )
        .unwrap();
        let mut unique_rows = 0;
        while let Some(batch) = unique.next_batch().unwrap() {
            unique_rows += batch.num_rows();
        }
        assert_eq!(unique_rows, 10_000);
        assert!(unique_spill.snapshot().peak_bytes > 0);
        assert_eq!(unique_pool.snapshot().current_bytes, 0);
    }

    fn join_rows(operator: &mut dyn BatchOperator) -> Vec<(Option<i64>, Option<i64>)> {
        let mut rows = Vec::new();
        while let Some(batch) = operator.next_batch().unwrap() {
            let left = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            let right = batch
                .column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            rows.extend((0..batch.num_rows()).map(|i| {
                (
                    (!left.is_null(i)).then(|| left.value(i)),
                    (!right.is_null(i)).then(|| right.value(i)),
                )
            }));
        }
        rows.sort();
        rows
    }

    #[test]
    fn adaptive_and_spilled_joins_drain_every_streaming_output_batch() {
        for adaptive_bytes in [0, 512 * 1024] {
            let pool = QueryMemoryPool::new("streaming-partition-join", 4 * 1024 * 1024).unwrap();
            let spill = spill();
            let mut right = vec![Some(1); 9_000];
            right.push(Some(2));
            let mut join = PartitionedHashJoin::new(
                input(vec![Some(1), Some(1), Some(1), None], 4),
                input(right, 9_001),
                JoinType::Full,
                vec![("id".into(), "id".into())],
                None,
                None,
                pool.operator("join").unwrap(),
                spill.clone(),
                8,
            )
            .unwrap();
            join.adaptive_bytes = Some(adaptive_bytes);
            let rows = join_rows(&mut join);
            assert_eq!(rows.len(), 27_002);
            assert_eq!(
                rows.iter()
                    .filter(|(left, right)| *left == Some(1) && *right == Some(1))
                    .count(),
                27_000
            );
            assert!(rows.contains(&(None, None)));
            assert!(rows.contains(&(None, Some(2))));
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().peak_bytes > 0, adaptive_bytes == 0);
        }
    }

    #[test]
    fn large_probe_with_small_build_streams_without_spill_run_explosion() {
        let pool = QueryMemoryPool::new("stream-large-probe", 32 * 1024 * 1024).unwrap();
        let spill = spill();
        let mut join = PartitionedHashJoin::new(
            input(
                (0..200_000).map(|value| Some(value % 1_000)).collect(),
                1_000,
            ),
            input((0..1_000).map(Some).collect(), 1_000),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        let mut rows = 0;
        while let Some(batch) = join.next_batch().unwrap() {
            rows += batch.num_rows();
        }
        assert_eq!(rows, 200_000);
        assert_eq!(spill.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn partitioned_join_matches_all_join_modes_with_nulls_and_duplicates() {
        let left = vec![Some(1), Some(1), Some(2), None];
        let right = vec![Some(1), Some(1), Some(3), None];
        for kind in [
            JoinType::Inner,
            JoinType::Left,
            JoinType::Right,
            JoinType::Full,
            JoinType::Cross,
        ] {
            let keys = if kind == JoinType::Cross {
                vec![]
            } else {
                vec![("id".into(), "id".into())]
            };
            let mut reference = HashJoin::try_new(
                input(left.clone(), 2),
                input(right.clone(), 2),
                kind,
                keys.clone(),
            )
            .unwrap();
            let pool = QueryMemoryPool::new("partition-join", 128 * 1024).unwrap();
            let spill = spill();
            let mut partitioned = PartitionedHashJoin::new(
                input(left.clone(), 2),
                input(right.clone(), 2),
                kind,
                keys,
                None,
                None,
                pool.operator("join").unwrap(),
                spill.clone(),
                8,
            )
            .unwrap();
            partitioned.adaptive_bytes = Some(0);
            assert_eq!(
                join_rows(&mut partitioned),
                join_rows(&mut reference),
                "{kind:?}"
            );
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
            assert!(spill.snapshot().peak_bytes > 0);
        }
    }

    #[test]
    fn grouped_spill_succeeds_when_unpartitioned_state_exceeds_budget() {
        let values = (0..2000).map(Some).collect::<Vec<_>>();
        let pool = QueryMemoryPool::new("partition-aggregate", 64 * 1024).unwrap();
        let expressions = vec![AggExpr::new(AggFunc::Count, "*")];
        let mut unpartitioned = HashAggregate::new_with_memory(
            input(values.clone(), 50),
            vec!["id".into()],
            expressions.clone(),
            pool.operator("unpartitioned").unwrap(),
        )
        .unwrap();
        assert!(unpartitioned.next_batch().is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
        let spill = spill();
        let mut aggregate = PartitionedHashAggregate::new(
            input(values, 50),
            vec!["id".into()],
            expressions,
            pool.operator("partitioned").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        let mut groups = 0;
        while let Some(batch) = aggregate.next_batch().unwrap() {
            groups += batch.num_rows();
            let counts = batch
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            assert!(counts.values().iter().all(|n| *n == 1));
        }
        assert_eq!(groups, 2000);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= 64 * 1024);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    #[test]
    fn skewed_join_fails_closed_and_cleans_all_partitions() {
        let pool = QueryMemoryPool::new("skew", 64 * 1024).unwrap();
        let spill = spill();
        let mut join = PartitionedHashJoin::new(
            input(vec![Some(1); 500], 25),
            input(vec![Some(1); 500], 25),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        assert!(join.next_batch().is_err());
        assert!(join.next_batch().unwrap().is_none());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    struct CountedInput {
        source: Box<dyn BatchOperator>,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl BatchOperator for CountedInput {
        fn schema(&self) -> &SchemaRef {
            self.source.schema()
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.source.next_batch()
        }
    }

    #[test]
    fn adaptive_prefix_caps_retained_batches_and_preserves_the_unread_tail() {
        let pool = QueryMemoryPool::new("prefix-bound", 1024 * 1024).unwrap();
        let memory = pool.operator("prefix").unwrap();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Box::new(CountedInput {
            source: input((0..100).map(Some).collect(), 1),
            reads: reads.clone(),
        });
        let prefix = BufferedPrefix::collect(source, &memory, 512 * 1024).unwrap();
        assert!(!prefix.complete);
        assert_eq!(prefix.batches.len(), MAX_ADAPTIVE_BATCHES);
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::Relaxed),
            MAX_ADAPTIVE_BATCHES
        );
        assert!(pool.snapshot().current_bytes <= 512 * 1024);
        let mut replay = prefix.replay();
        let mut values = Vec::new();
        while let Some(batch) = replay.next_batch().unwrap() {
            values.extend(
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .iter(),
            );
        }
        assert_eq!(values, (0..100).map(Some).collect::<Vec<_>>());
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 101);
        drop(replay);
        assert_eq!(pool.snapshot().current_bytes, 0);

        // A batch crossing the byte limit must go straight to spill, even if
        // it would be the final batch. Never read ahead to qualify a trial.
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Box::new(CountedInput {
            source: input(vec![Some(1); 100], 100),
            reads: reads.clone(),
        });
        let prefix = BufferedPrefix::collect(source, &memory, 1).unwrap();
        assert!(!prefix.complete);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(pool.snapshot().current_bytes, 0);
        drop(prefix);
    }

    #[test]
    fn adaptive_upstream_execution_and_io_errors_are_never_retried() {
        struct ErrorInput {
            source: Box<dyn BatchOperator>,
            error: Option<KaveonError>,
            reads: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl BatchOperator for ErrorInput {
            fn schema(&self) -> &SchemaRef {
                self.source.schema()
            }
            fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
                self.reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match self.source.next_batch()? {
                    Some(batch) => Ok(Some(batch)),
                    None => Err(self
                        .error
                        .take()
                        .expect("upstream error must not be retried")),
                }
            }
        }
        for error in [
            KaveonError::Execution("memory limit: this is a semantic error, not admission".into()),
            KaveonError::Io(std::io::Error::other("upstream failed")),
        ] {
            let pool = QueryMemoryPool::new("upstream-error", 128 * 1024).unwrap();
            let disk = spill();
            let expected = error.to_string();
            let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let mut aggregate = PartitionedHashAggregate::new(
                Box::new(ErrorInput {
                    source: input(vec![Some(1)], 1),
                    error: Some(error),
                    reads: reads.clone(),
                }),
                vec!["id".into()],
                vec![AggExpr::new(AggFunc::Count, "*")],
                pool.operator("aggregate").unwrap(),
                disk.clone(),
                16,
            )
            .unwrap();
            assert_eq!(aggregate.next_batch().unwrap_err().to_string(), expected);
            assert!(aggregate.next_batch().unwrap().is_none());
            assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 2);
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(disk.snapshot().peak_bytes, 0);
        }
    }

    #[test]
    fn adaptive_small_aggregate_and_join_avoid_disk_and_retain_output_guards() {
        let pool = QueryMemoryPool::new("adaptive-small", 128 * 1024).unwrap();
        let disk = spill();
        let mut aggregate = PartitionedHashAggregate::new(
            input(vec![Some(1), Some(1), None], 3),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("aggregate").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        assert_eq!(aggregate.next_batch().unwrap().unwrap().num_rows(), 2);
        assert!(pool.snapshot().current_bytes > 0);
        assert!(aggregate.next_batch().unwrap().is_none());
        for kind in [
            JoinType::Inner,
            JoinType::Left,
            JoinType::Right,
            JoinType::Full,
            JoinType::Cross,
        ] {
            let keys = if kind == JoinType::Cross {
                vec![]
            } else {
                vec![("id".into(), "id".into())]
            };
            let values = vec![None, Some(1), Some(1)];
            let mut reference = HashJoin::try_new(
                input(values.clone(), 3),
                input(values.clone(), 3),
                kind,
                keys.clone(),
            )
            .unwrap();
            let mut join = PartitionedHashJoin::new(
                input(values.clone(), 3),
                input(values, 3),
                kind,
                keys,
                None,
                None,
                pool.operator("join").unwrap(),
                disk.clone(),
                16,
            )
            .unwrap();
            assert_eq!(join_rows(&mut join), join_rows(&mut reference));
        }
        assert_eq!(disk.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn adaptive_memory_failure_replays_buffers_without_reading_upstream_twice() {
        let pool = QueryMemoryPool::new("adaptive-replay", 128 * 1024).unwrap();
        let disk = spill();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Box::new(CountedInput {
            source: input((0..1000).map(Some).collect(), 1000),
            reads: reads.clone(),
        });
        let mut aggregate = PartitionedHashAggregate::new(
            source,
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("aggregate").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        aggregate.adaptive_bytes = Some(32 * 1024);
        let mut rows = 0;
        while let Some(batch) = aggregate.next_batch().unwrap() {
            rows += batch.num_rows();
        }
        assert_eq!(rows, 1000);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert!(disk.snapshot().peak_bytes > 0);
        assert_eq!(disk.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);

        let disk = spill();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = || {
            Box::new(CountedInput {
                source: input((0..1000).map(|n| Some(n % 500)).collect(), 1000),
                reads: reads.clone(),
            }) as Box<dyn BatchOperator>
        };
        let mut join = PartitionedHashJoin::new(
            source(),
            source(),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        join.adaptive_bytes = Some(32 * 1024);
        let mut rows = 0;
        while let Some(batch) = join.next_batch().unwrap() {
            rows += batch.num_rows();
        }
        assert_eq!(rows, 2000);
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 4);
        assert!(disk.snapshot().peak_bytes > 0);
        assert_eq!(disk.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn adaptive_does_not_retry_semantic_or_cancellation_errors_and_enforces_disk_quota() {
        let pool = QueryMemoryPool::new("adaptive-errors", 128 * 1024).unwrap();
        let disk = spill();
        let mut overflow = PartitionedHashAggregate::new(
            input(vec![Some(i64::MAX); 2], 2),
            vec![],
            vec![AggExpr::new(AggFunc::Sum, "id")],
            pool.operator("overflow").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        assert!(
            overflow
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
        assert_eq!(disk.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);

        let tiny =
            SpillManager::new(std::env::temp_dir().join("kaveon-adaptive-tests"), 32).unwrap();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Box::new(CountedInput {
            source: input((0..1000).map(Some).collect(), 1000),
            reads: reads.clone(),
        });
        let mut aggregate = PartitionedHashAggregate::new(
            source,
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("quota").unwrap(),
            tiny.clone(),
            16,
        )
        .unwrap();
        aggregate.adaptive_bytes = Some(32 * 1024);
        assert!(
            aggregate
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("spill limit")
        );
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(tiny.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);

        // Cancel at EOF, after the prefix has been retained but before trial
        // execution. Do not depend on an operator's internal polling frequency.
        struct CancelAtEnd {
            source: Box<dyn BatchOperator>,
            canceled: Arc<std::sync::atomic::AtomicBool>,
        }
        impl BatchOperator for CancelAtEnd {
            fn schema(&self) -> &SchemaRef {
                self.source.schema()
            }
            fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
                let batch = self.source.next_batch()?;
                if batch.is_none() {
                    self.canceled
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(batch)
            }
        }
        let canceled_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = canceled_flag.clone();
        pool.set_cancellation_probe(move || observed.load(std::sync::atomic::Ordering::Acquire))
            .unwrap();
        let mut canceled = PartitionedHashAggregate::new(
            Box::new(CancelAtEnd {
                source: input(vec![Some(1); 10], 10),
                canceled: canceled_flag,
            }),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("cancel").unwrap(),
            disk.clone(),
            16,
        )
        .unwrap();
        assert!(
            canceled
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("query canceled")
        );
        assert_eq!(disk.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn global_empty_aggregate_produces_one_zero_count() {
        let pool = QueryMemoryPool::new("empty", 64 * 1024).unwrap();
        let mut aggregate = PartitionedHashAggregate::new(
            input(vec![], 10),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("aggregate").unwrap(),
            spill(),
            16,
        )
        .unwrap();
        let batch = aggregate.next_batch().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            0
        );
        assert!(aggregate.next_batch().unwrap().is_none());
    }

    #[test]
    fn disk_limit_and_early_drop_release_spill_runs() {
        let pool = QueryMemoryPool::new("disk", 128 * 1024).unwrap();
        let tiny =
            SpillManager::new(std::env::temp_dir().join("kaveon-partition-tests"), 32).unwrap();
        let mut aggregate = PartitionedHashAggregate::new(
            input(vec![Some(1)], 1),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("aggregate").unwrap(),
            tiny.clone(),
            4,
        )
        .unwrap();
        aggregate.adaptive_bytes = Some(0);
        assert!(aggregate.next_batch().is_err());
        assert_eq!(tiny.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
        let spill = spill();
        let mut aggregate = PartitionedHashAggregate::new(
            input((0..100).map(Some).collect(), 10),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("drop").unwrap(),
            spill.clone(),
            4,
        )
        .unwrap();
        assert!(aggregate.next_batch().unwrap().is_some());
        assert!(spill.snapshot().current_bytes > 0);
        drop(aggregate);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }
}
