//! Disk-partitioned hash execution. Each partition must fit the query's memory
//! budget: pathological key skew fails closed rather than repartitioning forever.
//! This bounds retained run metadata and open readers, not upstream allocations
//! or Arrow IPC codec scratch space. Callers must account retained output batches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{collections::VecDeque, sync::Arc};

use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use kaveon_core::{
    BatchOperator, Expr, KaveonError, MemoryReservation, OperatorMemoryAccount, QueryMemoryPool,
    Result,
};

use crate::{
    aggregate::{
        AggExpr, HashAggregate, PartialBatchEncoder, aggregate_metrics, aggregate_output_types,
        grouped_aggregate_states_to_schema_batch,
    },
    exchange::HashPartitioner,
    join::{BuiltSide, HashJoin, JoinType},
    semijoin::{BuildFacts, SemiJoinOperator},
    spill::{SpillManager, SpillRun, SpillRunReader},
};

const MAX_PARTITIONS: usize = 256;
const MAX_RUNS_PER_PARTITION: usize = 16;
const MAX_ADAPTIVE_BATCHES: usize = 64;

/// How a grouped partial decides whether aggregating pays. Registered on
/// the query pool (a test's, or the settings a caller pins), else read
/// from the environment once per operator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptivePartialSettings {
    /// Off: the partial aggregates every row, as before.
    pub enabled: bool,
    /// Rows a round must have read before its reduction is judged: a
    /// short round says little about the input.
    pub min_rows: u64,
    /// Groups per row at or above which a round "did not reduce".
    pub threshold: f64,
}

/// Rows read before a round's reduction counts. A round is the first
/// flush's worth of groups — a sixth of the budget share — so on any
/// budget that matters the sample is millions of rows, which is what
/// tells a uniform million-key input (a reduction the first hundred
/// thousand rows cannot see) from a unique one; the floor is for small
/// budgets, where a round of fewer rows is not judged at all.
pub const ADAPTIVE_PARTIAL_MIN_ROWS: u64 = 100_000;

/// Groups per row at or above which the partial stops aggregating.
/// Aggregating a row costs a probe of a table beyond the caches (P) and,
/// per group it leaves, the encoding at the flush plus the exchange and
/// the final merge of one partial row (E + X); passing a row through
/// costs E + X once. Aggregating pays while P + r(E + X) < E + X, that
/// is while the reduction r is below 1 − P / (E + X). Measured on the
/// q19 shape (`partial_stage_rate`, release, one thread): P ≈ 70 ns per
/// row against E ≈ 77 ns through the cleared table, and the exchange and
/// merge of a partial row are several hundred nanoseconds more (the
/// final merges at 134–146 ns per row, `merge_rate`; the AKS record puts
/// the output handling of q19's partial stage above a microsecond per
/// row), so P / (E + X) is below a fifth and the rule turns at four
/// groups in five rows — where Trino's partial gives up too. The
/// constant is deliberately on the aggregating side of the measurement.
pub const ADAPTIVE_PARTIAL_THRESHOLD: f64 = 0.8;

/// Rounds of pass-through per aggregating round: four after the first
/// decision, eight after the next, so a partial that stopped reducing
/// re-checks on at most a ninth of its rows and one that starts reducing
/// again (skew later in the input) is back to aggregating within eight
/// rounds.
const PASSTHROUGH_ROUNDS_FIRST: u64 = 4;
const PASSTHROUGH_ROUNDS_MAX: u64 = 8;

const ADAPTIVE_PARTIAL_RESOURCE: &str = "kaveon.exec.adaptive-partial.v1";

impl AdaptivePartialSettings {
    /// The settings a query runs with: the pool's, else the environment's
    /// (`KAVEON_ADAPTIVE_PARTIAL_AGGREGATION`, `on` unless `off`).
    pub fn for_query(memory: &QueryMemoryPool) -> Result<Self> {
        if let Some(settings) =
            memory.shared_resource_if_present::<Self>(ADAPTIVE_PARTIAL_RESOURCE)?
        {
            return Ok(*settings);
        }
        let enabled = match std::env::var("KAVEON_ADAPTIVE_PARTIAL_AGGREGATION") {
            Ok(value) if value.eq_ignore_ascii_case("off") => false,
            Ok(value) if value.eq_ignore_ascii_case("on") => true,
            Ok(_) => {
                return Err(KaveonError::Execution(
                    "KAVEON_ADAPTIVE_PARTIAL_AGGREGATION must be on or off".into(),
                ));
            }
            Err(std::env::VarError::NotPresent) => true,
            Err(_) => {
                return Err(KaveonError::Execution(
                    "KAVEON_ADAPTIVE_PARTIAL_AGGREGATION must contain valid Unicode".into(),
                ));
            }
        };
        Ok(Self {
            enabled,
            min_rows: ADAPTIVE_PARTIAL_MIN_ROWS,
            threshold: ADAPTIVE_PARTIAL_THRESHOLD,
        })
    }

    /// Pin these settings on the pool for every partial of the query.
    pub fn register(self, memory: &QueryMemoryPool) -> Result<()> {
        memory.shared_resource(ADAPTIVE_PARTIAL_RESOURCE, || Ok(self))?;
        Ok(())
    }
}

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
    // A spill already attached to the query (by an earlier operator, or a
    // test) is the query's, whatever the environment says.
    if let Some(resource) =
        memory.shared_resource_if_present::<(SpillManager, usize)>("kaveon.exec.hash-spill.v1")?
    {
        return Ok(Some((resource.0.clone(), resource.1)));
    }
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

/// Attach a spill to the query: every spill-capable operator of the
/// query uses it, whatever the environment says. Refused once one is
/// attached.
pub fn register_spill(
    memory: &QueryMemoryPool,
    spill: SpillManager,
    partitions: usize,
) -> Result<()> {
    validate_partitions(partitions)?;
    if memory
        .shared_resource_if_present::<(SpillManager, usize)>("kaveon.exec.hash-spill.v1")?
        .is_some()
    {
        return Err(KaveonError::Execution(
            "the query already has a spill attached".into(),
        ));
    }
    memory.shared_resource("kaveon.exec.hash-spill.v1", || Ok((spill, partitions)))?;
    Ok(())
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
pub(crate) struct RunSource {
    schema: SchemaRef,
    runs: VecDeque<SpillRun>,
    reader: Option<SpillRunReader>,
}

impl RunSource {
    pub(crate) fn new(schema: SchemaRef, runs: Vec<SpillRun>) -> Self {
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
    partition_input_with_flush(
        input,
        keys,
        count,
        memory,
        spill,
        input_reserved,
        adaptive_limit(memory)?,
    )
}

fn partition_input_with_flush(
    input: &mut dyn BatchOperator,
    keys: &[String],
    count: usize,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
    input_reserved: bool,
    flush_bytes: u64,
) -> Result<Vec<Vec<SpillRun>>> {
    let schema = Arc::clone(input.schema());
    let partitioner = if keys.is_empty() {
        None
    } else {
        Some(HashPartitioner::try_new_salted(
            &schema,
            keys,
            count,
            crate::exchange::SPILL_PARTITION_SALT,
        )?)
    };
    let mut split = |batch: &RecordBatch| match &partitioner {
        Some(partitioner) => partitioner.partition(batch),
        None => Ok(vec![batch.clone()]),
    };
    partition_input_by(
        input,
        &mut split,
        count,
        memory,
        spill,
        input_reserved,
        flush_bytes,
    )
}

/// `partition_input_with_flush` under one repartitioning level's salt.
fn partition_input_salted(
    input: &mut dyn BatchOperator,
    keys: &[String],
    count: usize,
    salt: u64,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
    flush_bytes: u64,
) -> Result<Vec<Vec<SpillRun>>> {
    let partitioner = HashPartitioner::try_new_salted(input.schema(), keys, count, salt)?;
    let mut split = |batch: &RecordBatch| partitioner.partition(batch);
    partition_input_by(input, &mut split, count, memory, spill, false, flush_bytes)
}

/// Spool an input into `count` partitions of runs, each batch split by
/// `split` (one batch per partition, empty ones allowed), buffering
/// several batches per run.
fn partition_input_by(
    input: &mut dyn BatchOperator,
    split: &mut dyn FnMut(&RecordBatch) -> Result<Vec<RecordBatch>>,
    count: usize,
    memory: &OperatorMemoryAccount,
    spill: &SpillManager,
    input_reserved: bool,
    flush_bytes: u64,
) -> Result<Vec<Vec<SpillRun>>> {
    let schema = Arc::clone(input.schema());
    let mut partitions: Vec<Vec<SpillRun>> = (0..count).map(|_| Vec::new()).collect();
    let mut buffered: Vec<Vec<RecordBatch>> = (0..count).map(|_| Vec::new()).collect();
    let mut buffered_memory = Vec::new();
    let mut buffered_bytes = 0_u64;
    // Batch several upstream partitions into each Arrow stream. The old path
    // created one tiny run per non-empty partition and upstream batch, then
    // paid to compact those files repeatedly. Retaining the existing
    // conservative preflight reservations keeps this buffer query-bounded.
    let flush_bytes = flush_bytes.max(1);
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
        let batches = split(&batch)?;
        drop(batch);
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
    /// Operators sharing the query budget side by side (parallel threads).
    budget_share: usize,
    /// The grouped partial once it flushes in rounds.
    flushing: Option<FlushingPartialAggregate>,
}

/// A source shared between the operator that owns it and the flush rounds
/// that read it.
type SharedSource = Rc<RefCell<Option<Box<dyn BatchOperator>>>>;

/// Yields a source's batches until the operator holds `flush_at` bytes,
/// then reports end of input, so the aggregate over it finishes its groups
/// as one partial batch. Partial groups merge across batches, so a grouped
/// partial never needs the disk: when memory is used up it flushes what it
/// has to the exchange and starts over on the rest of the input. Every
/// round yields at least one batch, so progress does not depend on the
/// budget. The source itself is shared with the operator that owns it and
/// resumes from where the round stopped.
struct FlushingSource {
    inner: SharedSource,
    exhausted: Rc<Cell<bool>>,
    schema: SchemaRef,
    memory: OperatorMemoryAccount,
    flush_at: u64,
    reserve_input: bool,
    current: Option<MemoryReservation>,
    yielded: usize,
    /// Rows this round has read.
    rows: Rc<Cell<u64>>,
    /// What the account held when the round began — the previous round's
    /// output batch, still on its way downstream — so the threshold
    /// measures this round's groups alone.
    baseline: Option<u64>,
}

impl BatchOperator for FlushingSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.current = None;
        let held = self.memory.snapshot().current_bytes;
        let baseline = *self.baseline.get_or_insert(held);
        if self.yielded > 0 && held.saturating_sub(baseline) >= self.flush_at {
            return Ok(None);
        }
        let mut inner = self.inner.borrow_mut();
        let Some(source) = inner.as_mut() else {
            return Ok(None);
        };
        match source.next_batch()? {
            Some(batch) => {
                if self.reserve_input {
                    self.current = Some(self.memory.reserve(batch.get_array_memory_size() as u64)?);
                }
                self.yielded += 1;
                self.rows.set(self.rows.get() + batch.num_rows() as u64);
                Ok(Some(batch))
            }
            None => {
                *inner = None;
                self.exhausted.set(true);
                Ok(None)
            }
        }
    }
}

/// What the grouped partial is doing with its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialMode {
    /// Aggregating into a table, flushing it in rounds.
    Aggregating,
    /// Passing rows through as their own partial rows for `remaining`
    /// more rows, then judging an aggregating round again.
    PassThrough { remaining: u64 },
}

/// A grouped partial aggregate that flushes on memory pressure. Partial
/// groups merge across batches, so a grouped partial never needs the disk:
/// it aggregates until it holds its share of the query budget, hands its
/// groups to the exchange as one partial batch, and resumes on the rest of
/// the input. Every round reads at least one batch, so progress does not
/// depend on the budget; a batch the budget cannot hold at all fails
/// closed as before.
///
/// The partial measures its reduction as it goes: a round that read at
/// least `min_rows` and made a group for `threshold` or more of them did
/// not reduce, and the next rows go through as their own partial rows
/// (`PartialBatchEncoder`: per-row states, the same encoding, one small
/// table cleared per batch) rather than into a table that only grows —
/// the final stage does the only aggregation that matters. Every
/// pass-through window ends with an aggregating round that judges again,
/// so a partial whose input starts reducing (skew later in the input)
/// resumes aggregating. Exactness is unchanged either way: the final
/// merges the same states.
pub struct FlushingPartialAggregate {
    source: SharedSource,
    exhausted: Rc<Cell<bool>>,
    input_schema: SchemaRef,
    schema: SchemaRef,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    group_types: Vec<arrow::datatypes::DataType>,
    output_types: Vec<arrow::datatypes::DataType>,
    memory: OperatorMemoryAccount,
    budget_share: usize,
    input_reserved: bool,
    settings: AdaptivePartialSettings,
    mode: PartialMode,
    /// Rounds of pass-through the next decision buys.
    passthrough_rounds: u64,
    encoder: Option<PartialBatchEncoder>,
    passthrough_rows: u64,
}

impl FlushingPartialAggregate {
    pub fn new(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
        memory: OperatorMemoryAccount,
    ) -> Result<Self> {
        let input_schema = Arc::clone(source.schema());
        // Validate the bindings once, before any input is read.
        let probe = HashAggregate::new(
            Box::new(RunSource::new(Arc::clone(&input_schema), Vec::new())),
            group_by.clone(),
            aggregates.clone(),
        )?;
        let group_types = probe.exchanged_key_types()?;
        let output_types = probe.output_types()?;
        let schema =
            grouped_aggregate_states_to_schema_batch(&[], &group_types, &output_types)?.schema();
        let settings = AdaptivePartialSettings::for_query(memory.query())?;
        Ok(Self {
            source: Rc::new(RefCell::new(Some(source))),
            exhausted: Rc::new(Cell::new(false)),
            input_schema,
            schema,
            group_by,
            aggregates,
            group_types,
            output_types,
            memory,
            budget_share: 1,
            input_reserved: false,
            settings,
            mode: PartialMode::Aggregating,
            passthrough_rounds: PASSTHROUGH_ROUNDS_FIRST,
            encoder: None,
            passthrough_rows: 0,
        })
    }

    /// The source holds a reservation for each batch it emits.
    pub fn with_reserved_input(mut self) -> Self {
        self.input_reserved = true;
        self
    }

    /// One of `share` operators on the same query budget.
    pub fn with_budget_share(mut self, share: usize) -> Self {
        self.budget_share = share.max(1);
        self
    }

    /// What the partial is doing with its rows now.
    pub fn mode(&self) -> PartialMode {
        self.mode
    }

    /// Rows passed through so far.
    pub fn passthrough_rows(&self) -> u64 {
        self.passthrough_rows
    }

    /// Judge a finished aggregating round: `rows` in, `groups` out. A
    /// round cut short by the end of the input is not judged (nothing
    /// follows it); one below `min_rows` says too little.
    fn judge_round(&mut self, rows: u64, groups: u64) {
        if !self.settings.enabled || self.exhausted.get() || rows < self.settings.min_rows {
            return;
        }
        if groups as f64 >= self.settings.threshold * rows as f64 {
            self.mode = PartialMode::PassThrough {
                remaining: rows.saturating_mul(self.passthrough_rounds).max(1),
            };
            self.passthrough_rounds = (self.passthrough_rounds * 2).min(PASSTHROUGH_ROUNDS_MAX);
        } else {
            self.passthrough_rounds = PASSTHROUGH_ROUNDS_FIRST;
        }
    }

    /// One aggregating round: the table until the flush threshold or the
    /// end of the input, as one partial batch (empty for no rows).
    fn aggregating_round(&mut self) -> Result<RecordBatch> {
        let rows = Rc::new(Cell::new(0_u64));
        let source = FlushingSource {
            inner: self.source.clone(),
            exhausted: self.exhausted.clone(),
            schema: Arc::clone(&self.input_schema),
            memory: self.memory.clone(),
            flush_at: self.flush_bytes(),
            reserve_input: !self.input_reserved,
            current: None,
            yielded: 0,
            rows: rows.clone(),
            baseline: None,
        };
        let operator = HashAggregate::new_with_memory(
            Box::new(source),
            self.group_by.clone(),
            self.aggregates.clone(),
            self.memory.clone(),
        )?
        .with_reserved_input();
        let (batch, state_memory) =
            operator.into_partial_batch(&self.group_types, &self.output_types)?;
        // The groups are gone once the batch exists; the consumer
        // accounts for the batch it takes, as for any operator's output.
        drop(state_memory);
        let rows = rows.get();
        let groups = batch.num_rows() as u64;
        aggregate_metrics(self.memory.query())?.record_partial_round(rows, groups, 0);
        self.judge_round(rows, groups);
        Ok(batch)
    }

    /// One batch of the input as its own partial rows.
    fn passthrough_batch(&mut self, remaining: u64) -> Result<Option<RecordBatch>> {
        let batch = {
            let mut inner = self.source.borrow_mut();
            let Some(source) = inner.as_mut() else {
                return Ok(None);
            };
            match source.next_batch()? {
                Some(batch) => batch,
                None => {
                    *inner = None;
                    self.exhausted.set(true);
                    return Ok(None);
                }
            }
        };
        let _input = if self.input_reserved {
            None
        } else {
            Some(self.memory.reserve(batch.get_array_memory_size() as u64)?)
        };
        self.memory.check_cancelled()?;
        if self.encoder.is_none() {
            self.encoder = Some(PartialBatchEncoder::new(
                &self.input_schema,
                self.group_by.clone(),
                self.aggregates.clone(),
                Some(self.memory.clone()),
            )?);
        }
        let encoder = self.encoder.as_mut().expect("encoder built");
        let (encoded, guard) = encoder.encode(&batch)?;
        // As for the aggregating round: the consumer accounts for the
        // batch it takes.
        drop(guard);
        let rows = batch.num_rows() as u64;
        self.passthrough_rows += rows;
        aggregate_metrics(self.memory.query())?.record_partial_round(
            rows,
            encoded.num_rows() as u64,
            rows,
        );
        self.mode = match remaining.saturating_sub(rows) {
            0 => PartialMode::Aggregating,
            remaining => PartialMode::PassThrough { remaining },
        };
        Ok(Some(encoded))
    }

    /// Bytes held before a flush: a sixth of the query budget, divided
    /// among the operators running side by side. A flush emits every group
    /// held once more, so the fewer the rounds the less the exchange
    /// carries; but a round's peak is about four times its state (the
    /// table's last doubling stays reserved, and the encoded batch is
    /// bigger than the columns it comes from), which is what the rest of
    /// the share is for.
    fn flush_bytes(&self) -> u64 {
        let pool = self.memory.query().snapshot().limit_bytes;
        (pool / 6 / self.budget_share as u64).max(1)
    }
}

impl BatchOperator for FlushingPartialAggregate {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if self.exhausted.get() {
                // The table the pass-through kept is not needed again.
                self.encoder = None;
                return Ok(None);
            }
            match self.mode {
                PartialMode::Aggregating => {
                    let batch = self.aggregating_round()?;
                    // An ungrouped partial over no rows is still one row
                    // of empty states; a grouped one is nothing.
                    if batch.num_rows() > 0 {
                        return Ok(Some(batch));
                    }
                }
                PartialMode::PassThrough { remaining } => {
                    match self.passthrough_batch(remaining)? {
                        Some(batch) if batch.num_rows() > 0 => return Ok(Some(batch)),
                        Some(_) => {}
                        None => {
                            self.encoder = None;
                            return Ok(None);
                        }
                    }
                }
            }
        }
    }
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
            budget_share: 1,
            flushing: None,
        })
    }

    /// The source must retain a reservation for each emitted input batch until
    /// the next source call. Partition copies and execution state remain charged.
    pub fn with_reserved_input(mut self) -> Self {
        self.input_reserved = true;
        self
    }

    /// This operator is one of `share` running side by side on the same
    /// query budget: its adaptive buffer is that fraction of the usual one,
    /// so together they hold what one would.
    pub fn with_budget_share(mut self, share: usize) -> Result<Self> {
        let limit = adaptive_limit(&self.memory)?;
        self.adaptive_bytes = Some((limit / share.max(1) as u64).max(1));
        self.budget_share = share.max(1);
        Ok(self)
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
        // Keys leave the operator as their logical values (a dictionary
        // column as its value type), the same as the in-memory aggregate.
        self.group_by
            .iter()
            .map(|name| {
                self.input_schema
                    .field_with_name(name)
                    .map(|field| crate::aggregate::exchanged_group_key_type(field.data_type()))
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
            let (batch, _state_memory) = operator.into_partial_batch(
                &self.group_types()?,
                &aggregate_output_types(&self.aggregates, &self.input_schema)?,
            )?;
            Some(batch)
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
            // A grouped partial that is flushing in rounds continues there.
            if let Some(flushing) = self.flushing.as_mut() {
                let batch = flushing.next_batch()?;
                if batch.is_none() {
                    self.flushing = None;
                }
                return Ok(batch);
            }
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
            // Grouped partials merge across batches: aggregate until the
            // budget share is used, flush the groups as one partial batch,
            // resume on the rest of the input. No probe, no disk.
            let mut flushing = FlushingPartialAggregate::new(
                input,
                self.group_by.clone(),
                self.aggregates.clone(),
                self.memory.clone(),
            )?
            .with_budget_share(self.budget_share);
            if self.input_reserved {
                flushing = flushing.with_reserved_input();
            }
            let batch = flushing.next_batch()?;
            if batch.is_some() {
                self.flushing = Some(flushing);
            }
            return Ok(batch);
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
            self.partitions = partition_input_with_flush(
                input.as_mut(),
                &self.group_by,
                self.count,
                &self.memory,
                &self.spill,
                self.input_reserved,
                self.adaptive_bytes.unwrap_or(adaptive_limit(&self.memory)?),
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

/// Repartitionings a join partition may take before its build must fit.
/// A partition at this depth is one of `count^(depth + 1)` — 65 536 at
/// the default sixteen — and a build that still does not fit there is
/// key skew no partitioning resolves: the join fails closed with the
/// budget's message rather than repartitioning forever.
pub const MAX_JOIN_SPILL_DEPTH: u32 = 3;

const JOIN_SPILL_METRICS_RESOURCE: &str = "kaveon.exec.join-spill-metrics.v1";

/// What the joins of one query wrote to the spill: bytes of runs,
/// partitions written (non-empty, at every depth) and the deepest
/// repartitioning any of them took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoinSpillSnapshot {
    pub bytes_written: u64,
    pub partitions: u64,
    pub max_depth: u64,
}

#[derive(Default)]
pub struct JoinSpillMetrics {
    bytes_written: AtomicU64,
    partitions: AtomicU64,
    max_depth: AtomicU64,
}

impl JoinSpillMetrics {
    #[must_use]
    pub fn snapshot(&self) -> JoinSpillSnapshot {
        JoinSpillSnapshot {
            bytes_written: self.bytes_written.load(Ordering::Acquire),
            partitions: self.partitions.load(Ordering::Acquire),
            max_depth: self.max_depth.load(Ordering::Acquire),
        }
    }

    fn record(&self, runs: &[Vec<SpillRun>], depth: u32) {
        let bytes = runs
            .iter()
            .flatten()
            .map(SpillRun::bytes)
            .fold(0_u64, u64::saturating_add);
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.partitions.fetch_add(
            runs.iter().filter(|runs| !runs.is_empty()).count() as u64,
            Ordering::Relaxed,
        );
        self.max_depth
            .fetch_max(u64::from(depth), Ordering::Relaxed);
    }
}

/// The query's join spill counters.
pub fn join_spill_metrics(memory: &QueryMemoryPool) -> Result<Arc<JoinSpillMetrics>> {
    memory.shared_resource(JOIN_SPILL_METRICS_RESOURCE, || {
        Ok(JoinSpillMetrics::default())
    })
}

/// The salt of one repartitioning level: the first level is the
/// aggregate's spill salt, every deeper level its own, so a partition's
/// sub-partitions spread instead of all landing in one.
fn spill_salt(depth: u32) -> u64 {
    if depth == 0 {
        crate::exchange::SPILL_PARTITION_SALT
    } else {
        crate::exchange::mix(
            crate::exchange::SPILL_PARTITION_SALT
                .wrapping_add(u64::from(depth).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        ) | 1
    }
}

/// The level a refused build partitions to: the first for a whole
/// input, one deeper for a partition, and the failure past the limit.
fn next_depth(depth: Option<u32>, count: usize, operator: &str, error: &str) -> Result<u32> {
    match depth {
        None => Ok(0),
        Some(depth) if depth < MAX_JOIN_SPILL_DEPTH => Ok(depth + 1),
        Some(depth) => Err(KaveonError::MemoryLimit(format!(
            "{operator} build side partition does not fit the memory budget after {depth} repartitionings ({}-way): {error}",
            (count as u64).saturating_pow(depth + 1)
        ))),
    }
}

/// One partition of a join's two inputs on disk, and how many
/// repartitionings made it.
struct JoinPartition {
    left: Vec<SpillRun>,
    right: Vec<SpillRun>,
    depth: u32,
}

/// A build the budget refused: what was collected (a refused batch
/// unaccounted at the end), the unread rest, and why.
struct Refusal {
    batches: Vec<RecordBatch>,
    guards: Vec<Option<MemoryReservation>>,
    tail: Option<Box<dyn BatchOperator>>,
    error: String,
}

impl Refusal {
    /// The collected batches and their rest as one source again.
    fn replay(self, schema: SchemaRef) -> Box<dyn BatchOperator> {
        Box::new(ReplayInput {
            schema,
            batches: self.batches.into(),
            tail: self.tail,
            guards: self.guards.into(),
            active_guard: None,
        })
    }
}

/// Every batch of a build side, each held by its guard.
type Collected = (Vec<RecordBatch>, Vec<Option<MemoryReservation>>);

/// A semi join's refused build: the probe input back, and the refusal.
type SemiRefusal = (Box<dyn BatchOperator>, Refusal);

/// Collect a build side while the budget admits it. A build is refused
/// when a batch's reservation is, or when what is held would leave the
/// probe less than half of the budget that was free when the build
/// began: `footprint` says what the built form of the held bytes and
/// rows will take, and the probe side, the output and the operators
/// downstream have to run beside it.
fn collect_build(
    mut source: Box<dyn BatchOperator>,
    memory: &OperatorMemoryAccount,
    footprint: impl Fn(u64, u64) -> u64,
) -> Result<std::result::Result<Collected, Refusal>> {
    let snapshot = memory.query().snapshot();
    let ceiling = snapshot.limit_bytes.saturating_sub(snapshot.current_bytes) / 2;
    let mut batches = Vec::new();
    let mut guards = Vec::new();
    let mut bytes = 0_u64;
    let mut rows = 0_u64;
    loop {
        memory.check_cancelled()?;
        let Some(batch) = source.next_batch()? else {
            return Ok(Ok((batches, guards)));
        };
        if batch.num_rows() == 0 {
            continue;
        }
        let batch_bytes = batch.get_array_memory_size() as u64;
        bytes = bytes.saturating_add(batch_bytes);
        rows = rows.saturating_add(batch.num_rows() as u64);
        match memory.reserve(batch_bytes) {
            Ok(guard) => {
                batches.push(batch);
                guards.push(Some(guard));
            }
            Err(KaveonError::MemoryLimit(error)) => {
                batches.push(batch);
                guards.push(None);
                return Ok(Err(Refusal {
                    batches,
                    guards,
                    tail: Some(source),
                    error,
                }));
            }
            Err(error) => return Err(error),
        }
        let needed = footprint(bytes, rows);
        if needed > ceiling {
            return Ok(Err(Refusal {
                batches,
                guards,
                tail: Some(source),
                error: format!(
                    "a build side of {bytes} bytes in {rows} rows needs {needed} bytes, more than half of the {} bytes free",
                    ceiling.saturating_mul(2)
                ),
            }));
        }
    }
}

/// A hash join whose build side spills when the budget refuses it. The
/// right input is collected while the budget admits it and built in
/// memory (`BuiltSide`); a refusal — of a batch's reservation, of the
/// concatenated copy, of the index, or of more than half of the free
/// budget — partitions both inputs by the join keys' hash (salted, as the
/// aggregate's spill is) into `count` partitions on disk, and each
/// partition is built and probed on its own within the budget, its
/// memory exact through the operator's account. A partition whose build
/// is refused in turn is repartitioned under the next level's salt, to
/// `MAX_JOIN_SPILL_DEPTH`; after that it fails closed. Inner, left, right
/// and full joins partition alike: a key's rows are all in one partition,
/// so each partition's unmatched rows are the join's. A cross join has no
/// key to partition by and fails closed at the first refusal. A broadcast
/// join builds the same table on every task, so each task spills its own.
pub struct PartitionedHashJoin {
    left: Option<Box<dyn BatchOperator>>,
    right: Option<Box<dyn BatchOperator>>,
    left_schema: SchemaRef,
    right_schema: SchemaRef,
    schema: SchemaRef,
    join_type: JoinType,
    keys: Vec<(String, String)>,
    key_indices: Vec<(usize, usize)>,
    left_qualifier: Option<String>,
    right_qualifier: Option<String>,
    memory: OperatorMemoryAccount,
    spill: SpillManager,
    count: usize,
    partitions: VecDeque<JoinPartition>,
    failed: bool,
    active: Option<HashJoin>,
    metrics: Arc<JoinSpillMetrics>,
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
        let key_indices = crate::join::resolve_join_keys(&left_schema, &right_schema, &keys)?;
        let metrics = join_spill_metrics(memory.query())?;
        Ok(Self {
            left: Some(left),
            right: Some(right),
            left_schema,
            right_schema,
            schema: Arc::clone(probe.schema()),
            join_type,
            keys,
            key_indices,
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
            metrics,
        })
    }

    /// The build in memory, or what to partition.
    fn build(
        &self,
        right: Box<dyn BatchOperator>,
    ) -> Result<std::result::Result<BuiltSide, Refusal>> {
        // The built form: the concatenated copy beside the batches, and
        // the index's key copy and per-row overhead.
        let (batches, guards) = match collect_build(right, &self.memory, |bytes, rows| {
            bytes
                .saturating_mul(2)
                .saturating_add(rows.saturating_mul(64))
        })? {
            Ok(collected) => collected,
            Err(refused) => return Ok(Err(refused)),
        };
        match BuiltSide::build(
            &self.right_schema,
            &batches,
            &self.key_indices,
            self.join_type,
            Some(&self.memory),
        ) {
            Ok(built) => {
                drop(guards);
                Ok(Ok(built))
            }
            Err(KaveonError::MemoryLimit(error)) => Ok(Err(Refusal {
                batches,
                guards,
                tail: None,
                error,
            })),
            Err(error) => Err(error),
        }
    }

    fn probe(&self, left: Box<dyn BatchOperator>, built: BuiltSide) -> Result<HashJoin> {
        HashJoin::try_new_built(
            left,
            Arc::clone(&self.right_schema),
            built,
            self.join_type,
            self.keys.clone(),
            self.left_qualifier.as_deref(),
            self.right_qualifier.as_deref(),
            Some(self.memory.clone()),
        )
    }

    /// Both inputs to disk, partitioned by the keys under `depth`'s salt.
    fn partition_pair(
        &self,
        mut left: Box<dyn BatchOperator>,
        mut right: Box<dyn BatchOperator>,
        depth: u32,
    ) -> Result<VecDeque<JoinPartition>> {
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
        let flush = adaptive_limit(&self.memory)?;
        let left_runs = partition_input_salted(
            left.as_mut(),
            &left_keys,
            self.count,
            spill_salt(depth),
            &self.memory,
            &self.spill,
            flush,
        )?;
        let right_runs = partition_input_salted(
            right.as_mut(),
            &right_keys,
            self.count,
            spill_salt(depth),
            &self.memory,
            &self.spill,
            flush,
        )?;
        self.metrics.record(&left_runs, depth);
        self.metrics.record(&right_runs, depth);
        Ok(left_runs
            .into_iter()
            .zip(right_runs)
            .map(|(left, right)| JoinPartition { left, right, depth })
            .collect())
    }

    /// The refusal of a build with `left` still in hand: partition the
    /// pair one level deeper, or fail closed at the depth limit.
    fn repartition(
        &mut self,
        left: Box<dyn BatchOperator>,
        refused: Refusal,
        depth: Option<u32>,
    ) -> Result<()> {
        if self.join_type == JoinType::Cross {
            return Err(KaveonError::MemoryLimit(format!(
                "cross join build side does not fit the memory budget and cannot be partitioned: {}",
                refused.error
            )));
        }
        let next = next_depth(depth, self.count, "join", &refused.error)?;
        let right = refused.replay(Arc::clone(&self.right_schema));
        let partitions = self.partition_pair(left, right, next)?;
        // A repartitioned partition's pieces are next, so the runs of a
        // partition are consumed before another's are opened.
        for partition in partitions.into_iter().rev() {
            self.partitions.push_front(partition);
        }
        Ok(())
    }

    fn execute_next(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(active) = &mut self.active {
                if let Some(batch) = active.next_batch()? {
                    return Ok(Some(batch));
                }
                self.active = None;
            }
            if let Some(left) = self.left.take() {
                let right = self
                    .right
                    .take()
                    .expect("right input exists before the build");
                match self.build(right)? {
                    Ok(built) => self.active = Some(self.probe(left, built)?),
                    Err(refused) => self.repartition(left, refused, None)?,
                }
                continue;
            }
            let Some(partition) = self.partitions.pop_front() else {
                return Ok(None);
            };
            if partition.left.is_empty() && partition.right.is_empty() {
                continue;
            }
            let left: Box<dyn BatchOperator> = Box::new(RunSource::new(
                Arc::clone(&self.left_schema),
                partition.left,
            ));
            let right: Box<dyn BatchOperator> = Box::new(RunSource::new(
                Arc::clone(&self.right_schema),
                partition.right,
            ));
            match self.build(right)? {
                Ok(built) => self.active = Some(self.probe(left, built)?),
                Err(refused) => self.repartition(left, refused, Some(partition.depth))?,
            }
        }
    }
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

/// A semi or anti join operator: spill-safe under the query's spill,
/// the in-memory operator otherwise.
#[allow(clippy::too_many_arguments)]
pub fn semi_join(
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    left_key: Expr,
    right_key: Expr,
    anti: bool,
    residual: Option<Expr>,
    memory: Option<OperatorMemoryAccount>,
) -> Result<Box<dyn BatchOperator>> {
    if let Some(memory) = memory {
        if let Some((spill, count)) = spill_from_environment(memory.query())? {
            return Ok(Box::new(PartitionedSemiJoin::new(
                left, right, left_key, right_key, anti, residual, memory, spill, count,
            )?));
        }
        let mut operator = SemiJoinOperator::new(left, right, left_key, right_key, anti)?;
        if let Some(residual) = residual {
            operator = operator.with_residual(residual)?;
        }
        return Ok(Box::new(operator.with_memory(memory)));
    }
    let mut operator = SemiJoinOperator::new(left, right, left_key, right_key, anti)?;
    if let Some(residual) = residual {
        operator = operator.with_residual(residual)?;
    }
    Ok(Box::new(operator))
}

/// A semi or anti join whose build side spills when the budget refuses
/// it, as `PartitionedHashJoin` does: the right input is collected while
/// the budget admits it and built (a key set, or the rows by key under a
/// residual); a refusal partitions both inputs by the hash of the key
/// expression's value — the same equality the build uses, so a
/// dictionary and a plain column, or two integer widths, land together —
/// and each partition is built and probed on its own, repartitioned to
/// `MAX_JOIN_SPILL_DEPTH` when refused again. NOT IN's rules read the
/// whole right input, so the partitioning records whether any right key
/// was NULL and whether any was not, and every partition is probed with
/// those facts.
pub struct PartitionedSemiJoin {
    left: Option<Box<dyn BatchOperator>>,
    right: Option<Box<dyn BatchOperator>>,
    left_schema: SchemaRef,
    right_schema: SchemaRef,
    left_key: Expr,
    right_key: Expr,
    anti: bool,
    residual: Option<Expr>,
    memory: OperatorMemoryAccount,
    spill: SpillManager,
    count: usize,
    partitions: VecDeque<JoinPartition>,
    facts: BuildFacts,
    failed: bool,
    active: Option<SemiJoinOperator>,
    metrics: Arc<JoinSpillMetrics>,
}

impl PartitionedSemiJoin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        left_key: Expr,
        right_key: Expr,
        anti: bool,
        residual: Option<Expr>,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
        partition_count: usize,
    ) -> Result<Self> {
        validate_partitions(partition_count)?;
        let left_schema = Arc::clone(left.schema());
        let right_schema = Arc::clone(right.schema());
        // Validate the shape once, and take the keys as the operator
        // normalises them (`*` to the column, a numeric cast), so the
        // partitioning evaluates what the build compares.
        let mut probe = SemiJoinOperator::new(
            Box::new(RunSource::new(Arc::clone(&left_schema), Vec::new())),
            Box::new(RunSource::new(Arc::clone(&right_schema), Vec::new())),
            left_key,
            right_key,
            anti,
        )?;
        if let Some(residual) = &residual {
            probe = probe.with_residual(residual.clone())?;
        }
        let (left_key, right_key, _, _) = probe.shape();
        let metrics = join_spill_metrics(memory.query())?;
        Ok(Self {
            left: Some(left),
            right: Some(right),
            left_schema,
            right_schema,
            left_key,
            right_key,
            anti,
            residual,
            memory,
            spill,
            count: partition_count,
            partitions: VecDeque::new(),
            facts: BuildFacts::default(),
            failed: false,
            active: None,
            metrics,
        })
    }

    fn operator(
        &self,
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        partitioned: bool,
    ) -> Result<SemiJoinOperator> {
        let mut operator = SemiJoinOperator::new(
            left,
            right,
            self.left_key.clone(),
            self.right_key.clone(),
            self.anti,
        )?;
        if let Some(residual) = &self.residual {
            operator = operator.with_residual(residual.clone())?;
        }
        operator = operator.with_memory(self.memory.clone());
        if partitioned {
            operator = operator.with_build_facts(self.facts);
        }
        Ok(operator)
    }

    /// The build in memory with `left` as its probe, or `left` back with
    /// what to partition.
    fn build(
        &self,
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        partitioned: bool,
    ) -> Result<std::result::Result<SemiJoinOperator, SemiRefusal>> {
        // The built form beside the batches: the key set or the retained
        // rows, at most the batches again with an entry per row.
        let (batches, guards) = match collect_build(right, &self.memory, |bytes, rows| {
            bytes
                .saturating_mul(2)
                .saturating_add(rows.saturating_mul(128))
        })? {
            Ok(collected) => collected,
            Err(refused) => return Ok(Err((left, refused))),
        };
        let trial: Box<dyn BatchOperator> = Box::new(ReplayInput {
            schema: Arc::clone(&self.right_schema),
            batches: batches.clone().into(),
            tail: None,
            guards: VecDeque::new(),
            active_guard: None,
        });
        let mut operator = self.operator(left, trial, partitioned)?;
        match operator.build() {
            Ok(()) => {
                drop(guards);
                Ok(Ok(operator))
            }
            Err(KaveonError::MemoryLimit(error)) => {
                let (left, _) = operator.into_inputs();
                Ok(Err((
                    left,
                    Refusal {
                        batches,
                        guards,
                        tail: None,
                        error,
                    },
                )))
            }
            Err(error) => Err(error),
        }
    }

    /// Both inputs to disk, partitioned by the key's value under
    /// `depth`'s salt; the first level records the right input's facts.
    fn partition_pair(
        &mut self,
        mut left: Box<dyn BatchOperator>,
        mut right: Box<dyn BatchOperator>,
        depth: u32,
    ) -> Result<VecDeque<JoinPartition>> {
        let flush = adaptive_limit(&self.memory)?;
        let salt = spill_salt(depth);
        let count = self.count;
        let memory = self.memory.clone();
        let left_runs = crate::expr_eval::with_expression_memory(Some(&memory), || {
            let key = &self.left_key;
            let mut split = |batch: &RecordBatch| split_by_key(batch, key, count, salt, None);
            partition_input_by(
                left.as_mut(),
                &mut split,
                count,
                &memory,
                &self.spill,
                false,
                flush,
            )
        })?;
        let mut facts = if depth == 0 {
            Some(BuildFacts::default())
        } else {
            None
        };
        let right_runs = crate::expr_eval::with_expression_memory(Some(&memory), || {
            let key = &self.right_key;
            let mut split =
                |batch: &RecordBatch| split_by_key(batch, key, count, salt, facts.as_mut());
            partition_input_by(
                right.as_mut(),
                &mut split,
                count,
                &memory,
                &self.spill,
                false,
                flush,
            )
        })?;
        if let Some(facts) = facts {
            self.facts = facts;
        }
        self.metrics.record(&left_runs, depth);
        self.metrics.record(&right_runs, depth);
        Ok(left_runs
            .into_iter()
            .zip(right_runs)
            .map(|(left, right)| JoinPartition { left, right, depth })
            .collect())
    }

    fn repartition(
        &mut self,
        left: Box<dyn BatchOperator>,
        refused: Refusal,
        depth: Option<u32>,
    ) -> Result<()> {
        let next = next_depth(depth, self.count, "semi join", &refused.error)?;
        let right = refused.replay(Arc::clone(&self.right_schema));
        let partitions = self.partition_pair(left, right, next)?;
        for partition in partitions.into_iter().rev() {
            self.partitions.push_front(partition);
        }
        Ok(())
    }

    fn execute_next(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(active) = &mut self.active {
                if let Some(batch) = active.next_batch()? {
                    return Ok(Some(batch));
                }
                self.active = None;
            }
            if let Some(left) = self.left.take() {
                let right = self
                    .right
                    .take()
                    .expect("right input exists before the build");
                match self.build(left, right, false)? {
                    Ok(operator) => self.active = Some(operator),
                    Err((left, refused)) => self.repartition(left, refused, None)?,
                }
                continue;
            }
            let Some(partition) = self.partitions.pop_front() else {
                return Ok(None);
            };
            // A partition with no left rows emits nothing; one with no
            // right rows still probes (an anti join keeps its rows).
            if partition.left.is_empty() {
                continue;
            }
            let left: Box<dyn BatchOperator> = Box::new(RunSource::new(
                Arc::clone(&self.left_schema),
                partition.left,
            ));
            let right: Box<dyn BatchOperator> = Box::new(RunSource::new(
                Arc::clone(&self.right_schema),
                partition.right,
            ));
            match self.build(left, right, true)? {
                Ok(operator) => self.active = Some(operator),
                Err((left, refused)) => self.repartition(left, refused, Some(partition.depth))?,
            }
        }
    }
}

impl BatchOperator for PartitionedSemiJoin {
    fn schema(&self) -> &SchemaRef {
        &self.left_schema
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

/// A batch split into `count` partitions by the hash of `key`'s value
/// per row (NULL keys to the first), recording in `facts` whether any
/// key was NULL and whether any was not.
fn split_by_key(
    batch: &RecordBatch,
    key: &Expr,
    count: usize,
    salt: u64,
    mut facts: Option<&mut BuildFacts>,
) -> Result<Vec<RecordBatch>> {
    let values = crate::expr_eval::evaluate(key, batch)?;
    let mut indices = (0..count)
        .map(|_| arrow::array::UInt32Builder::new())
        .collect::<Vec<_>>();
    for row in 0..batch.num_rows() {
        if row % 1024 == 0 {
            crate::expr_eval::check_expression_cancelled()?;
        }
        let partition = match crate::semijoin::key_hash_at(values.as_ref(), row)? {
            Some(hash) => {
                if let Some(facts) = facts.as_deref_mut() {
                    facts.nonempty = true;
                }
                (crate::exchange::mix(hash ^ salt) % count as u64) as usize
            }
            None => {
                if let Some(facts) = facts.as_deref_mut() {
                    facts.has_null = true;
                }
                0
            }
        };
        indices[partition].append_value(u32::try_from(row).map_err(|_| {
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
                .map(|column| arrow::compute::take(column.as_ref(), &indices, None))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(RecordBatch::try_new(batch.schema(), columns)?)
        })
        .collect()
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
    use kaveon_core::{MemoryAdmissionController, QueryMemoryPool};
    use std::collections::HashMap;

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

        // 300 000 groups cannot stay in memory in a 16 MiB budget: the
        // partial flushes its groups in rounds instead of spilling — every
        // row is read once, every group reaches the output (as several
        // partial rows that merge), the peak stays inside the budget, and
        // the disk is not touched.
        let bounded_pool = QueryMemoryPool::new("bounded-partial", 16 * 1024 * 1024).unwrap();
        let bounded_spill = spill();
        let mut high_cardinality = PartitionedHashAggregate::new_partial(
            input((0..300_000).map(Some).collect(), 8_192),
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
        assert!(high_cardinality_batches.len() > 1, "several flush rounds");
        let states = grouped_aggregate_states_from_batches(&high_cardinality_batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        assert_eq!(merged.len(), 300_000);
        let input_rows = aggregate_metrics(&bounded_pool)
            .unwrap()
            .snapshot()
            .input_rows;
        assert_eq!(input_rows, 300_000, "every row read once");
        assert!(bounded_pool.snapshot().peak_bytes <= 16 * 1024 * 1024);
        assert_eq!(bounded_spill.snapshot().peak_bytes, 0);
        assert_eq!(bounded_pool.snapshot().current_bytes, 0);
    }

    /// Drain a flushing partial and merge its rows as the final would:
    /// (group count, total of the first COUNT, batches emitted).
    fn merged_partial(aggregate: &mut FlushingPartialAggregate) -> (usize, u64, usize) {
        let mut batches = Vec::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            batches.push(batch);
        }
        let states = grouped_aggregate_states_from_batches(&batches).unwrap();
        let merged = merge_grouped_aggregate_states(states).unwrap();
        let finalized = finalize_grouped_aggregate_states(&merged).unwrap();
        let total = finalized
            .iter()
            .map(|group| match group.values[0] {
                crate::aggregate::FinalAggregateValue::Count(value) => value,
                _ => panic!("expected count state"),
            })
            .sum::<u64>();
        (finalized.len(), total, batches.len())
    }

    fn adaptive(min_rows: u64) -> AdaptivePartialSettings {
        AdaptivePartialSettings {
            enabled: true,
            min_rows,
            threshold: ADAPTIVE_PARTIAL_THRESHOLD,
        }
    }

    #[test]
    fn a_partial_over_near_unique_keys_passes_rows_through_and_merges_exactly() {
        // 300 000 unique keys in a 16 MiB budget: the first round fills a
        // sixth of the budget and made a group for every row, so the
        // rows after it go through as their own partial rows; the merge
        // is what the aggregating partial would have given.
        let pool = QueryMemoryPool::new("passthrough-partial", 16 * 1024 * 1024).unwrap();
        adaptive(10_000).register(&pool).unwrap();
        let mut aggregate = FlushingPartialAggregate::new(
            input((0..300_000).map(Some).collect(), 8_192),
            vec!["id".into()],
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "id"),
                AggExpr::new(AggFunc::Max, "id"),
            ],
            pool.operator("partial").unwrap(),
        )
        .unwrap();
        let (groups, total, _) = merged_partial(&mut aggregate);
        assert_eq!((groups, total), (300_000, 300_000));
        let passed = aggregate.passthrough_rows();
        assert!(passed > 0, "the partial never stopped aggregating");
        assert!(passed < 300_000, "the first round aggregates");
        let metrics = aggregate_metrics(&pool).unwrap().snapshot();
        assert_eq!(metrics.partial_input_rows, 300_000);
        assert_eq!(metrics.partial_passthrough_rows, passed);
        assert_eq!(metrics.partial_output_rows, 300_000);
        assert_eq!(metrics.partial_reduction(), Some(1.0));
        assert!(pool.snapshot().peak_bytes <= 16 * 1024 * 1024);
        assert_eq!(pool.snapshot().current_bytes, 0);

        // The same rows with the rule off: every row through the table.
        let pool = QueryMemoryPool::new("aggregating-partial", 16 * 1024 * 1024).unwrap();
        AdaptivePartialSettings {
            enabled: false,
            ..adaptive(10_000)
        }
        .register(&pool)
        .unwrap();
        let mut aggregate = FlushingPartialAggregate::new(
            input((0..300_000).map(Some).collect(), 8_192),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("partial").unwrap(),
        )
        .unwrap();
        assert_eq!(merged_partial(&mut aggregate).0, 300_000);
        assert_eq!(aggregate.passthrough_rows(), 0);
        assert_eq!(aggregate.mode(), PartialMode::Aggregating);
        assert_eq!(
            aggregate_metrics(&pool)
                .unwrap()
                .snapshot()
                .partial_passthrough_rows,
            0
        );
    }

    #[test]
    fn a_partial_over_low_cardinality_keys_keeps_aggregating() {
        let pool = QueryMemoryPool::new("reducing-partial", 16 * 1024 * 1024).unwrap();
        adaptive(10_000).register(&pool).unwrap();
        let mut aggregate = FlushingPartialAggregate::new(
            input((0..300_000).map(|value| Some(value % 17)).collect(), 8_192),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("partial").unwrap(),
        )
        .unwrap();
        let (groups, total, batches) = merged_partial(&mut aggregate);
        assert_eq!((groups, total, batches), (17, 300_000, 1));
        assert_eq!(aggregate.passthrough_rows(), 0);
        let metrics = aggregate_metrics(&pool).unwrap().snapshot();
        assert_eq!(metrics.partial_output_rows, 17);
        assert!(metrics.partial_reduction().unwrap() < 0.001);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_partial_resumes_aggregating_when_its_input_starts_reducing() {
        // Unique keys first, then a hundred thousand rows over seventeen
        // keys: the partial stops aggregating on the unique prefix, judges
        // an aggregating round at the end of each pass-through window, and
        // is aggregating again by the end. The merge is exact throughout.
        let pool = QueryMemoryPool::new("skewed-partial", 16 * 1024 * 1024).unwrap();
        adaptive(10_000).register(&pool).unwrap();
        let values = (0..200_000)
            .map(Some)
            .chain((0..1_000_000).map(|value| Some(value % 17)))
            .collect();
        let mut aggregate = FlushingPartialAggregate::new(
            input(values, 8_192),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("partial").unwrap(),
        )
        .unwrap();
        let (groups, total, _) = merged_partial(&mut aggregate);
        assert_eq!((groups, total), (200_000, 1_200_000));
        let passed = aggregate.passthrough_rows();
        assert!(passed > 0, "the unique prefix stops the aggregation");
        // At most the first window (four rounds) and the re-check windows
        // over the reducing suffix; the bulk of the suffix aggregates.
        assert!(passed < 400_000, "{passed} rows passed through");
        assert_eq!(aggregate.mode(), PartialMode::Aggregating);
        let metrics = aggregate_metrics(&pool).unwrap().snapshot();
        assert!(metrics.partial_output_rows < 600_000);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_pass_through_partial_on_the_row_path_encodes_per_row_states() {
        // COUNT(DISTINCT) is not a columnar accumulator: the pass-through
        // encodes each batch through the row-path aggregate, and the
        // merge is still exact.
        let pool = QueryMemoryPool::new("row-path-passthrough", 16 * 1024 * 1024).unwrap();
        // The row path holds more per group, so its rounds are shorter.
        adaptive(2_000).register(&pool).unwrap();
        let mut aggregate = FlushingPartialAggregate::new(
            input((0..200_000).map(Some).collect(), 8_192),
            vec!["id".into()],
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Count, "id").distinct(),
            ],
            pool.operator("partial").unwrap(),
        )
        .unwrap();
        let (groups, total, _) = merged_partial(&mut aggregate);
        assert_eq!((groups, total), (200_000, 200_000));
        assert!(aggregate.passthrough_rows() > 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    /// The partial stage's cost per row on the shape ClickBench q19 hands
    /// it (near-unique Int64 + Int64 + Utf8 keys, one COUNT) and on a
    /// low-cardinality key, with the adaptive rule on and off. Ignored by
    /// default; run as `cargo test --release -p kaveon-exec
    /// partial_stage_rate -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark: prints the partial stage's rate, run explicitly in release"]
    fn partial_stage_rate() {
        use arrow::array::StringArray;
        const ROWS: usize = 4_000_000;
        const BATCH_ROWS: usize = 8_192;
        let schema = Arc::new(Schema::new(vec![
            Field::new("user", DataType::Int64, false),
            Field::new("minute", DataType::Int64, false),
            Field::new("phrase", DataType::Utf8, false),
        ]));
        let user = |i: usize| (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) as i64;
        let shape = |near_unique: bool| -> Vec<RecordBatch> {
            (0..ROWS / BATCH_ROWS)
                .map(|batch| {
                    let rows = batch * BATCH_ROWS..(batch + 1) * BATCH_ROWS;
                    let users = rows.clone().map(|i| {
                        if near_unique {
                            // Every sixteenth row repeats the one fifteen before it.
                            user(if i % 16 == 15 { i - 15 } else { i })
                        } else {
                            user(i % 1_000)
                        }
                    });
                    let minutes = rows.clone().map(|i| (i % 60) as i64);
                    let phrases = rows.map(|i| {
                        if i.is_multiple_of(5) {
                            String::new()
                        } else {
                            format!("search phrase {}", i % 100_000)
                        }
                    });
                    RecordBatch::try_new(
                        Arc::clone(&schema),
                        vec![
                            Arc::new(Int64Array::from_iter_values(users)),
                            Arc::new(Int64Array::from_iter_values(minutes)),
                            Arc::new(StringArray::from_iter_values(phrases)),
                        ],
                    )
                    .unwrap()
                })
                .collect()
        };
        for (name, near_unique) in [("near-unique", true), ("low-cardinality", false)] {
            let batches = shape(near_unique);
            for enabled in [false, true] {
                let mut best = std::time::Duration::MAX;
                let mut passed = 0;
                let mut output_rows = 0;
                for _ in 0..3 {
                    let pool = QueryMemoryPool::new("partial-rate", 1 << 30).unwrap();
                    AdaptivePartialSettings {
                        enabled,
                        ..adaptive(ADAPTIVE_PARTIAL_MIN_ROWS)
                    }
                    .register(&pool)
                    .unwrap();
                    let mut aggregate = FlushingPartialAggregate::new(
                        Box::new(Input {
                            schema: Arc::clone(&schema),
                            batches: batches.clone().into(),
                        }),
                        vec!["user".into(), "minute".into(), "phrase".into()],
                        vec![AggExpr::new(AggFunc::Count, "*")],
                        pool.operator("partial").unwrap(),
                    )
                    .unwrap();
                    let started = std::time::Instant::now();
                    let mut rows = 0;
                    while let Some(batch) = aggregate.next_batch().unwrap() {
                        rows += batch.num_rows();
                    }
                    best = best.min(started.elapsed());
                    passed = aggregate.passthrough_rows();
                    output_rows = rows;
                }
                println!(
                    "{name} keys, adaptive {}: {:.0} ns/row, {} partial rows out of {ROWS}, {passed} passed through",
                    if enabled { "on" } else { "off" },
                    best.as_nanos() as f64 / ROWS as f64,
                    output_rows
                );
            }
        }
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
        // The 9 001-row build needs about 720 KiB built: more than half
        // of a 1 MiB budget, so it spills (nine keys, so its partitions
        // fit); well inside a 4 MiB one.
        for (budget, spills) in [(1 << 20, true), (4 << 20, false)] {
            let pool = QueryMemoryPool::new("streaming-partition-join", budget).unwrap();
            let spill = spill();
            let mut right = (0..9_000).map(|value| Some(value % 9)).collect::<Vec<_>>();
            right.push(Some(20));
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
            let rows = join_rows(&mut join);
            // Three left rows of key 1 against a thousand: 3 000 pairs;
            // the 8 001 right rows of other keys and the NULL left row
            // come out unmatched.
            assert_eq!(rows.len(), 11_002);
            assert_eq!(
                rows.iter()
                    .filter(|(left, right)| *left == Some(1) && *right == Some(1))
                    .count(),
                3_000
            );
            assert!(rows.contains(&(None, None)));
            assert!(rows.contains(&(None, Some(20))));
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().peak_bytes > 0, spills);
            assert_eq!(
                join_spill_metrics(&pool).unwrap().snapshot().partitions > 0,
                spills
            );
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
        // Every keyed join type under a budget that refuses its build:
        // every key sixteen times on each side, half of the keys on one
        // side only, a NULL key in every eighth row — the rows are the
        // in-memory join's, and the disk was used. The 8 192-row build
        // needs some 650 KiB built, more than half of the 1 MiB budget;
        // an eighth of it fits.
        let keyed = |offset: i64| {
            (0..8_192)
                .map(|row| (row % 8 != 7).then_some(row % 512 + offset))
                .collect::<Vec<_>>()
        };
        let left = keyed(0);
        let right = keyed(256);
        for kind in [
            JoinType::Inner,
            JoinType::Left,
            JoinType::Right,
            JoinType::Full,
        ] {
            let keys = vec![("id".into(), "id".into())];
            let mut reference = HashJoin::try_new(
                input(left.clone(), 256),
                input(right.clone(), 256),
                kind,
                keys.clone(),
            )
            .unwrap();
            let pool = QueryMemoryPool::new("partition-join", 1 << 20).unwrap();
            let spill = spill();
            let mut partitioned = PartitionedHashJoin::new(
                input(left.clone(), 256),
                input(right.clone(), 256),
                kind,
                keys,
                None,
                None,
                pool.operator("join").unwrap(),
                spill.clone(),
                8,
            )
            .unwrap();
            assert_eq!(
                join_rows(&mut partitioned),
                join_rows(&mut reference),
                "{kind:?}"
            );
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
            assert!(spill.snapshot().peak_bytes > 0, "{kind:?} did not spill");
            let metrics = join_spill_metrics(&pool).unwrap().snapshot();
            assert!(
                metrics.partitions > 0 && metrics.bytes_written > 0,
                "{kind:?}"
            );
        }
        // A cross join has no key to partition by: in memory it matches
        // the reference, and a budget that refuses its build fails closed.
        let small_left = vec![Some(1), Some(1), Some(2), None];
        let small_right = vec![Some(1), Some(1), Some(3), None];
        let mut reference = HashJoin::try_new(
            input(small_left.clone(), 2),
            input(small_right.clone(), 2),
            JoinType::Cross,
            vec![],
        )
        .unwrap();
        let pool = QueryMemoryPool::new("cross-join", 128 * 1024).unwrap();
        let spill = spill();
        let mut cross = PartitionedHashJoin::new(
            input(small_left, 2),
            input(small_right, 2),
            JoinType::Cross,
            vec![],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            8,
        )
        .unwrap();
        assert_eq!(join_rows(&mut cross), join_rows(&mut reference));
        assert_eq!(spill.snapshot().peak_bytes, 0);
        let pool = QueryMemoryPool::new("cross-join-refused", 1 << 20).unwrap();
        let mut cross = PartitionedHashJoin::new(
            input(left, 256),
            input(right, 256),
            JoinType::Cross,
            vec![],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            8,
        )
        .unwrap();
        let error = cross.next_batch().unwrap_err().to_string();
        assert!(error.contains("cannot be partitioned"), "{error}");
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_skewed_partition_repartitions_to_the_depth_limit_and_names_it() {
        // Every key equal: no partitioning separates them, so the join
        // repartitions the one live partition to the limit and fails
        // closed with the depth in its message; every run is removed.
        let pool = QueryMemoryPool::new("skew-depth", 64 * 1024).unwrap();
        let spill = spill();
        let mut join = PartitionedHashJoin::new(
            input(vec![Some(7); 800], 50),
            input(vec![Some(7); 800], 50),
            JoinType::Left,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        let error = join.next_batch().unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "after {MAX_JOIN_SPILL_DEPTH} repartitionings ({}-way)",
                16_u64.pow(MAX_JOIN_SPILL_DEPTH + 1)
            )),
            "{error}"
        );
        let metrics = join_spill_metrics(&pool).unwrap().snapshot();
        assert_eq!(metrics.max_depth, u64::from(MAX_JOIN_SPILL_DEPTH));
        assert!(join.next_batch().unwrap().is_none());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_partitioned_join_output_matches_across_keys_that_share_a_hash_partition() {
        // Many distinct keys, both sides larger than the budget admits,
        // repeated on the probe: each partition builds within budget and
        // the union of the partitions is the in-memory join.
        let left = (0..6_000)
            .map(|value| Some(value % 1_500))
            .collect::<Vec<_>>();
        let right = (0..3_000)
            .map(|value| Some(value % 2_000))
            .collect::<Vec<_>>();
        let mut reference = HashJoin::try_new(
            input(left.clone(), 500),
            input(right.clone(), 500),
            JoinType::Full,
            vec![("id".into(), "id".into())],
        )
        .unwrap();
        let pool = QueryMemoryPool::new("partition-join-wide", 96 * 1024).unwrap();
        let spill = spill();
        let mut partitioned = PartitionedHashJoin::new(
            input(left, 500),
            input(right, 500),
            JoinType::Full,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        assert_eq!(join_rows(&mut partitioned), join_rows(&mut reference));
        let metrics = join_spill_metrics(&pool).unwrap().snapshot();
        assert!(metrics.partitions >= 16, "{metrics:?}");
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    fn semi_rows(operator: &mut dyn BatchOperator) -> Vec<Option<i64>> {
        let mut rows = Vec::new();
        while let Some(batch) = operator.next_batch().unwrap() {
            let ids = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            rows.extend((0..ids.len()).map(|i| (!ids.is_null(i)).then(|| ids.value(i))));
        }
        rows.sort();
        rows
    }

    #[test]
    fn a_partitioned_semi_or_anti_join_matches_the_in_memory_operator_under_a_refusing_budget() {
        use kaveon_core::Expr;
        let key = || Expr::Column("id".into());
        // Left: every key 0..1 500 four times and NULLs; right: 0..1 000
        // three times, 2 000.. once, and NULLs, so a key can be on one
        // side only, on both, or NULL on either — and NOT IN's rule for a
        // NULL in the set must hold across partitions.
        let left = (0..6_000)
            .map(|value| (value % 400 != 399).then_some(value % 1_500))
            .collect::<Vec<_>>();
        for right_nulls in [false, true] {
            let right = (0..4_000)
                .map(|value| {
                    if right_nulls && value % 500 == 499 {
                        None
                    } else if value < 3_000 {
                        Some(value % 1_000)
                    } else {
                        Some(value - 1_000)
                    }
                })
                .collect::<Vec<_>>();
            for anti in [false, true] {
                let mut reference = SemiJoinOperator::new(
                    input(left.clone(), 500),
                    input(right.clone(), 500),
                    key(),
                    key(),
                    anti,
                )
                .unwrap();
                let pool = QueryMemoryPool::new("partition-semi", 96 * 1024).unwrap();
                let spill = spill();
                let mut partitioned = PartitionedSemiJoin::new(
                    input(left.clone(), 500),
                    input(right.clone(), 500),
                    key(),
                    key(),
                    anti,
                    None,
                    pool.operator("semi").unwrap(),
                    spill.clone(),
                    16,
                )
                .unwrap();
                let expected = semi_rows(&mut reference);
                assert_eq!(
                    semi_rows(&mut partitioned),
                    expected,
                    "anti {anti}, right nulls {right_nulls}"
                );
                if anti && right_nulls {
                    assert!(expected.is_empty(), "a NULL in the set empties NOT IN");
                } else {
                    assert!(!expected.is_empty());
                }
                assert!(spill.snapshot().peak_bytes > 0, "did not spill");
                assert_eq!(pool.snapshot().current_bytes, 0);
                assert_eq!(spill.snapshot().current_bytes, 0);
            }
        }
        // In memory under a budget that admits the build: no disk.
        let pool = QueryMemoryPool::new("semi-in-memory", 8 << 20).unwrap();
        let spill = spill();
        let right = (0..4_000)
            .map(|value| Some(value % 1_000))
            .collect::<Vec<_>>();
        let mut reference = SemiJoinOperator::new(
            input(left.clone(), 500),
            input(right.clone(), 500),
            key(),
            key(),
            true,
        )
        .unwrap();
        let mut partitioned = PartitionedSemiJoin::new(
            input(left, 500),
            input(right, 500),
            key(),
            key(),
            true,
            None,
            pool.operator("semi").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        assert_eq!(semi_rows(&mut partitioned), semi_rows(&mut reference));
        assert_eq!(spill.snapshot().peak_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_partitioned_semi_join_with_a_residual_matches_the_in_memory_operator() {
        use arrow::array::StringArray;
        use kaveon_core::Expr;
        // Q21's shape: EXISTS (… WHERE o.k = t.k AND o.s <> t.s) and its
        // NOT EXISTS, over two-column inputs, the build refused.
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
        ]));
        let rows = |count: i64, keys: i64, suppliers: i64| -> Box<dyn BatchOperator> {
            let batches = (0..count)
                .collect::<Vec<_>>()
                .chunks(400)
                .map(|chunk| {
                    RecordBatch::try_new(
                        Arc::clone(&schema),
                        vec![
                            Arc::new(Int64Array::from_iter(
                                chunk
                                    .iter()
                                    .map(|value| (value % 97 != 96).then_some(value % keys)),
                            )),
                            Arc::new(StringArray::from_iter(
                                chunk
                                    .iter()
                                    .map(|value| Some(format!("supplier-{}", value % suppliers))),
                            )),
                        ],
                    )
                    .unwrap()
                })
                .collect::<VecDeque<_>>();
            Box::new(Input {
                schema: Arc::clone(&schema),
                batches,
            })
        };
        let residual = Expr::BinaryOp {
            left: Box::new(Expr::Column("s".into())),
            op: kaveon_core::BinaryOp::Ne,
            right: Box::new(Expr::Column("o_s".into())),
        };
        let rename = |source: Box<dyn BatchOperator>| -> Box<dyn BatchOperator> {
            Box::new(
                crate::project::ProjectOperator::new(
                    source,
                    vec![
                        Expr::Alias {
                            expr: Box::new(Expr::Column("k".into())),
                            name: "o_k".into(),
                        },
                        Expr::Alias {
                            expr: Box::new(Expr::Column("s".into())),
                            name: "o_s".into(),
                        },
                    ],
                )
                .unwrap(),
            )
        };
        for anti in [false, true] {
            let mut reference = SemiJoinOperator::new(
                rows(6_000, 1_200, 5),
                rename(rows(4_000, 1_500, 3)),
                Expr::Column("k".into()),
                Expr::Column("o_k".into()),
                anti,
            )
            .unwrap()
            .with_residual(residual.clone())
            .unwrap();
            let pool = QueryMemoryPool::new("partition-semi-residual", 128 * 1024).unwrap();
            let spill = spill();
            let mut partitioned = PartitionedSemiJoin::new(
                rows(6_000, 1_200, 5),
                rename(rows(4_000, 1_500, 3)),
                Expr::Column("k".into()),
                Expr::Column("o_k".into()),
                anti,
                Some(residual.clone()),
                pool.operator("semi").unwrap(),
                spill.clone(),
                16,
            )
            .unwrap();
            let expected = semi_rows(&mut reference);
            assert!(!expected.is_empty());
            assert_eq!(semi_rows(&mut partitioned), expected, "anti {anti}");
            assert!(spill.snapshot().peak_bytes > 0, "did not spill");
            assert!(join_spill_metrics(&pool).unwrap().snapshot().partitions > 0);
            assert_eq!(pool.snapshot().current_bytes, 0);
            assert_eq!(spill.snapshot().current_bytes, 0);
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
        // The bounded fallback must have exercised the disk path, then release
        // every run before returning. A zero current-byte count alone would
        // also pass if the operator silently rejected the input.
        assert!(spill.snapshot().runs_written > 0);
        assert!(spill.snapshot().bytes_written > 0);
        assert_eq!(spill.snapshot().current_bytes, 0);
    }

    #[test]
    fn partitioned_aggregate_groups_dictionary_keys_as_their_values() {
        // A dictionary-encoded key column, as a Parquet file with its Arrow
        // schema stored hands it over: the group column comes out as text.
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "country",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
            Field::new("actions", DataType::Int64, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(arrow::array::Int32DictionaryArray::new(
                    arrow::array::Int32Array::from(vec![Some(0), Some(1), Some(0), None]),
                    Arc::new(arrow::array::StringArray::from(vec![
                        "India", "Germany", "Unused",
                    ])),
                )),
                Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(4), Some(8)])),
            ],
        )
        .unwrap();
        let pool = QueryMemoryPool::new("dictionary-groups", 1024 * 1024).unwrap();
        let mut aggregate = PartitionedHashAggregate::new(
            Box::new(Input {
                schema,
                batches: std::collections::VecDeque::from([batch]),
            }),
            vec!["country".into()],
            vec![
                crate::aggregate::AggExpr::new(AggFunc::Count, "*"),
                crate::aggregate::AggExpr::new(AggFunc::Sum, "actions"),
            ],
            pool.operator("partitioned").unwrap(),
            spill(),
            16,
        )
        .unwrap();
        assert_eq!(aggregate.schema().field(0).data_type(), &DataType::Utf8);
        let mut rows = std::collections::BTreeMap::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .unwrap();
            let sums = batch
                .column(2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            for row in 0..batch.num_rows() {
                rows.insert(
                    (!keys.is_null(row)).then(|| keys.value(row).to_owned()),
                    sums.value(row),
                );
            }
        }
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[&Some("India".to_owned())], 5);
        assert_eq!(rows[&Some("Germany".to_owned())], 2);
        assert_eq!(rows[&None], 8);
    }

    #[test]
    fn multi_run_grouped_spill_preserves_exact_counts_and_cleans_runs() {
        let pool = QueryMemoryPool::new("multi-run-aggregate", 64 * 1024).unwrap();
        let spill = spill();
        let values = (0..16_384)
            .map(|value| Some((value % 256) as i64))
            .collect::<Vec<_>>();
        let mut aggregate = PartitionedHashAggregate::new(
            input(values, 64),
            vec!["id".into()],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.operator("aggregate").unwrap(),
            spill.clone(),
            16,
        )
        .unwrap();
        let mut counts = HashMap::new();
        while let Some(batch) = aggregate.next_batch().unwrap() {
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            let values = batch
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            for row in 0..batch.num_rows() {
                counts.insert(keys.value(row), values.value(row));
            }
        }
        assert_eq!(counts.len(), 256);
        assert!(counts.values().all(|count| *count == 64));
        let snapshot = spill.snapshot();
        assert!(snapshot.runs_written > 16, "{snapshot:?}");
        assert!(snapshot.compactions > 0, "{snapshot:?}");
        assert_eq!(snapshot.current_bytes, 0);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn concurrent_partitioned_queries_fail_closed_at_admission_and_remain_exact() {
        let controller = Arc::new(MemoryAdmissionController::new(128 * 1024).unwrap());
        let handles = (0..16)
            .map(|query| {
                let controller = Arc::clone(&controller);
                std::thread::spawn(move || {
                    let admitted = match controller.admit(format!("spill-query-{query}"), 64 * 1024)
                    {
                        Ok(admitted) => admitted,
                        Err(_) => return false,
                    };
                    let pool = admitted.pool().clone();
                    let mut aggregate = PartitionedHashAggregate::new(
                        input((0..2_048).map(|value| Some(value % 128)).collect(), 64),
                        vec!["id".into()],
                        vec![AggExpr::new(AggFunc::Count, "*")],
                        pool.operator("aggregate").unwrap(),
                        spill(),
                        8,
                    )
                    .unwrap();
                    let mut groups = 0;
                    while let Some(batch) = aggregate.next_batch().unwrap() {
                        groups += batch.num_rows();
                    }
                    groups == 128 && pool.snapshot().current_bytes == 0
                })
            })
            .collect::<Vec<_>>();
        let exact = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|exact| *exact)
            .count();
        assert_eq!(exact, 2);
        assert_eq!(controller.snapshot().current_bytes, 0);
        assert_eq!(controller.snapshot().peak_bytes, 128 * 1024);
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
        let error = join.next_batch().unwrap_err().to_string();
        assert!(
            error.contains("memory") || error.contains("skew"),
            "{error}"
        );
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
