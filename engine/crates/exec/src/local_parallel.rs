//! Parallel partial aggregation inside one task. Rows are hash-partitioned
//! by group key across aggregator threads, so every group lives in exactly
//! one thread's map and memory is the same as one aggregator's; ungrouped
//! aggregates round-robin. Operators are constructed inside their owning
//! threads.
//!
//! Sources read on threads of their own (`Sources::Threads`) hand each
//! batch to every thread with the reservation holding it: one charge per
//! batch in flight, the source's. When the budget refuses a source its
//! next batch, the source keeps the batch and asks the threads for memory
//! (`Pressure`); a thread answers between two batches — its input wakes
//! it for that — by giving memory up (the final merge spills its live
//! table) or declining, and the source tries again once memory was given
//! up. The refusal is final only when every thread has declined and none
//! gave memory up since the attempt. No thread waits with a lock held.
use crate::{
    aggregate::{
        AggExpr, HashAggregate, aggregate_output_types, exchanged_group_key_type,
        grouped_aggregate_states_to_schema_batch,
    },
    distinct::DistinctOperator,
    exchange::HashPartitioner,
    partitioned::{FlushingPartialAggregate, PartitionedHashAggregate, spill_from_environment},
    spill::SpillManager,
};
use arrow::{
    datatypes::{DataType, SchemaRef},
    record_batch::RecordBatch,
};
use kaveon_core::{
    BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, QueryMemoryPool, Result,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_WORKERS: usize = 16;
/// Aggregator threads per task when `KAVEON_LOCAL_PARALLELISM` is unset:
/// the cores the process sees, at most this many.
const DEFAULT_MAX_PARALLELISM: usize = 4;
pub fn configured_parallelism() -> Result<usize> {
    let value = match std::env::var("KAVEON_LOCAL_PARALLELISM") {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|_| error("KAVEON_LOCAL_PARALLELISM must be a positive integer"))?,
        Err(std::env::VarError::NotPresent) => thread::available_parallelism()
            .map_or(1, usize::from)
            .min(DEFAULT_MAX_PARALLELISM),
        Err(_) => return Err(error("KAVEON_LOCAL_PARALLELISM must contain Unicode")),
    };
    if value == 0 {
        return Err(error("KAVEON_LOCAL_PARALLELISM must be positive"));
    }
    Ok(value
        .min(MAX_WORKERS)
        .min(thread::available_parallelism().map_or(1, usize::from)))
}

// --- Per-query parallelism ceiling -------------------------------------------
// A statement may lower its own parallelism (`settings.local_parallelism`).
// The ceiling rides on the query memory pool, which every operator of the
// query already receives on every node that admitted a task for it, so the
// operators need no new argument: they ask the pool instead of the process.

/// The pool resource that carries a query's parallelism ceiling.
const QUERY_PARALLELISM_RESOURCE: &str = "kaveon.exec.local-parallelism.v1";

/// Caps every operator of the query on `pool` at `threads` aggregator
/// threads. Only lowers: a ceiling above the node's configured parallelism
/// has no effect. Set once per pool; a second, different value is an error.
pub fn set_query_parallelism(pool: &QueryMemoryPool, threads: usize) -> Result<()> {
    if threads == 0 {
        return Err(error("query parallelism must be positive"));
    }
    let ceiling = pool.shared_resource(QUERY_PARALLELISM_RESOURCE, || Ok(threads))?;
    if *ceiling != threads {
        return Err(error("query parallelism is already set for this query"));
    }
    Ok(())
}

/// Threads for one operator of the query on `pool`: the node's configured
/// parallelism, lowered to the query's ceiling when the statement set one.
/// Without a pool (embedded and test plans) the node's value stands.
pub fn query_parallelism(pool: Option<&QueryMemoryPool>) -> Result<usize> {
    let configured = configured_parallelism()?;
    let Some(pool) = pool else {
        return Ok(configured);
    };
    Ok(pool
        .shared_resource_if_present::<usize>(QUERY_PARALLELISM_RESOURCE)?
        .map_or(configured, |ceiling| configured.min(*ceiling)))
}
struct QueuedBatch {
    batch: RecordBatch,
    _memory: Held,
}

/// What a queued batch holds until it is consumed. One batch is held by
/// exactly one of these, cloned to every thread that takes the batch:
/// the reservation its source handed over with it, or the queue's charge
/// for a batch that came without one.
#[derive(Clone)]
enum Held {
    Reserved { _guard: Arc<MemoryReservation> },
    Charged { _charge: Arc<QueueCharge> },
}

/// Memory for the batches in the threads' input queues, kept once taken:
/// what the queues can hold at once — `QUEUE_BATCHES` source batches'
/// worth, up to a quarter of the query budget — is taken before the first
/// batch is partitioned, and the pump charges each part against it, the
/// part giving its bytes back when its thread is done with it. So the
/// pump does not ask the budget for a queue slot once the threads have
/// filled it: a thread that filled the budget spills or fails on its own
/// account, and the queue that feeds it does not fail with it. Batches
/// larger than the first, or than the quarter admits, reserve as they come.
struct QueueBudget {
    account: OperatorMemoryAccount,
    state: Mutex<QueueState>,
}

struct QueueState {
    guards: Vec<MemoryReservation>,
    held: u64,
    available: u64,
}

/// How many source batches the queues hold at once, at most: two queued
/// and one in hand per thread, whose parts make three batches, plus the
/// batch being partitioned — and one more, since a batch's parts are not
/// equal and the threads may hold the larger ones.
const QUEUE_BATCHES: u64 = 5;
/// The share of the query budget the queues take up front, at most.
const QUEUE_BUDGET_SHARE: u64 = 4;

impl QueueBudget {
    fn new(account: OperatorMemoryAccount) -> Self {
        Self {
            account,
            state: Mutex::new(QueueState {
                guards: Vec::new(),
                held: 0,
                available: 0,
            }),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, QueueState>> {
        self.state
            .lock()
            .map_err(|_| error("parallel input queue budget poisoned"))
    }

    /// Hold the queues' worth of a batch of `batch_bytes`, within the
    /// share of the budget the queues may take up front.
    fn ensure_for(&self, batch_bytes: u64) -> Result<()> {
        let share = self.account.query().snapshot().limit_bytes / QUEUE_BUDGET_SHARE;
        let bytes = batch_bytes.saturating_mul(QUEUE_BATCHES).min(share);
        let mut state = self.lock()?;
        if bytes > state.held {
            let guard = self.account.reserve(bytes - state.held)?;
            state.held += guard.bytes();
            state.available += guard.bytes();
            state.guards.push(guard);
        }
        Ok(())
    }

    /// Charge `bytes` against what is held, growing it when short.
    fn charge(self: &Arc<Self>, bytes: u64) -> Result<QueueCharge> {
        let mut state = self.lock()?;
        if bytes > state.available {
            let guard = self.account.reserve(bytes - state.available)?;
            state.held += guard.bytes();
            state.available += guard.bytes();
            state.guards.push(guard);
        }
        state.available -= bytes;
        Ok(QueueCharge {
            budget: Arc::clone(self),
            bytes,
        })
    }

    /// Charge `bytes` from a thread that can wait: grow what is held up
    /// to the queues' share of the budget, and past that wait for the
    /// threads to give parts back rather than take more of the budget —
    /// the back-pressure of a source read on its own thread. Leaves when
    /// the operator is stopped or the query cancelled.
    fn charge_waiting(self: &Arc<Self>, bytes: u64, stopped: &AtomicBool) -> Result<QueueCharge> {
        let share = self.account.query().snapshot().limit_bytes / QUEUE_BUDGET_SHARE;
        loop {
            {
                let mut state = self.lock()?;
                if bytes > state.available && state.held < share {
                    let growth = (bytes - state.available).min(share - state.held);
                    let guard = self.account.reserve(growth)?;
                    state.held += guard.bytes();
                    state.available += guard.bytes();
                    state.guards.push(guard);
                }
                if bytes <= state.available {
                    state.available -= bytes;
                    return Ok(QueueCharge {
                        budget: Arc::clone(self),
                        bytes,
                    });
                }
                if state.held == 0 || bytes > state.held {
                    // A batch the share cannot hold at all: the budget's
                    // answer, not a wait.
                    let guard = self.account.reserve(bytes - state.available)?;
                    state.held += guard.bytes();
                    state.available += guard.bytes();
                    state.guards.push(guard);
                    state.available -= bytes;
                    return Ok(QueueCharge {
                        budget: Arc::clone(self),
                        bytes,
                    });
                }
            }
            if stopped.load(Ordering::Acquire) {
                return Err(stopped_error());
            }
            self.account.check_cancelled()?;
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Bytes of the queue budget in use by one batch, given back on drop.
struct QueueCharge {
    budget: Arc<QueueBudget>,
    bytes: u64,
}

impl Drop for QueueCharge {
    fn drop(&mut self) {
        if let Ok(mut state) = self.budget.state.lock() {
            state.available += self.bytes;
        }
    }
}

/// One thread's input: the batches its queue receives, each held until
/// the next call. While a pressure request waits on this thread's answer
/// (`Pressure`), the input does not sleep on: it hands out an empty batch
/// of its schema instead, so the operator over it — which answers between
/// batches — gets to answer now. Only a thread whose operator took a
/// responder's ticket is woken that way; an operator that does not take
/// part never sees one.
struct ChannelInput {
    schema: SchemaRef,
    receiver: Receiver<QueuedBatch>,
    stopped: Arc<AtomicBool>,
    pressure: Option<(Arc<Pressure>, usize)>,
    current: Option<Held>,
}
impl BatchOperator for ChannelInput {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.current = None;
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return Err(stopped_error());
            }
            if let Some((pressure, index)) = &self.pressure
                && pressure.awaits(*index)
            {
                return Ok(Some(RecordBatch::new_empty(Arc::clone(&self.schema))));
            }
            match self.receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(queued) => {
                    self.current = Some(queued._memory);
                    return Ok(Some(queued.batch));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

/// Produces typed partials for the same final merger used by distributed execution.
/// What one thread runs over its share of the rows. The operator's
/// output batches are forwarded as they come.
pub type ThreadOperator = Arc<
    dyn Fn(
            Box<dyn BatchOperator>,
            &QueryMemoryPool,
            &ThreadContext,
        ) -> Result<Box<dyn BatchOperator>>
        + Send
        + Sync,
>;

/// What every thread of one parallel operator shares.
pub struct ThreadContext {
    /// This thread's place among the threads, from zero.
    pub index: usize,
    /// Threads running side by side on the query budget.
    pub workers: usize,
    /// The query's spill budget and partition count when spilling is on.
    pub spill: Option<(SpillManager, usize)>,
    /// Where the sources read on threads of their own ask the threads
    /// for memory (`Pressure`): an operator that can give some up takes
    /// a responder's ticket for its thread. None when the source is read
    /// on the calling thread.
    pub pressure: Option<Arc<Pressure>>,
}

/// A batch with the reservation that holds it: what a source read on its
/// own thread hands the pump. The reservation crosses with the batch and
/// is held by the threads' queues until the last thread is done with it,
/// so an in-flight batch is charged exactly once — by its source, when it
/// was decoded — never again by the queue. A batch without one is charged
/// to the queue's budget instead.
#[derive(Debug)]
pub struct ReservedBatch {
    pub batch: RecordBatch,
    pub memory: Option<MemoryReservation>,
}

impl ReservedBatch {
    /// A batch the pump is to charge for.
    pub fn unreserved(batch: RecordBatch) -> Self {
        Self {
            batch,
            memory: None,
        }
    }
}

/// A source read on a thread of its own, feeding every thread of the
/// operator through the pump. Its batches come with the reservations
/// holding them. A reservation the budget refuses is reported as
/// `KaveonError::MemoryLimit` **with the batch kept**: the next call
/// offers the same batch again, its reservation tried first, so the pump
/// can ask the operator's threads for memory and try again without a
/// row lost or read twice. That is the contract that lets the pump retry
/// at all; a source that cannot keep a batch must not report a refusal.
pub trait ThreadSource {
    fn schema(&self) -> &SchemaRef;
    fn next_batch(&mut self) -> Result<Option<ReservedBatch>>;
}

/// Any operator as a thread source: its batches come unreserved and the
/// pump charges its queue's budget for them. A refusal from such a source
/// is final — the operator's contract does not keep the batch.
pub struct Unreserved(pub Box<dyn BatchOperator>);

impl ThreadSource for Unreserved {
    fn schema(&self) -> &SchemaRef {
        self.0.schema()
    }
    fn next_batch(&mut self) -> Result<Option<ReservedBatch>> {
        Ok(self.0.next_batch()?.map(ReservedBatch::unreserved))
    }
}

/// Opens a source inside the thread that reads it: what crosses to that
/// thread is the means of opening, not the source.
pub type SourceOpener = Box<dyn FnOnce() -> Result<Box<dyn ThreadSource>> + Send>;

/// Where a parallel operator's rows come from.
pub enum Sources {
    /// One source, read on the calling thread.
    Here(Box<dyn BatchOperator>),
    /// Several sources of one schema, each opened and read on a thread of
    /// its own — the payloads of an exchange, one per producer, decoded
    /// side by side.
    Threads {
        schema: SchemaRef,
        openers: Vec<SourceOpener>,
    },
}

impl Sources {
    pub fn schema(&self) -> &SchemaRef {
        match self {
            Self::Here(source) => source.schema(),
            Self::Threads { schema, .. } => schema,
        }
    }

    /// The sources as one operator on the calling thread, read one after
    /// another; each batch's reservation is held until the next call, as
    /// any operator holds its output.
    pub fn into_operator(self) -> Result<Box<dyn BatchOperator>> {
        match self {
            Self::Here(source) => Ok(source),
            Self::Threads { schema, openers } => Ok(Box::new(ChainedSources {
                schema,
                openers: openers.into_iter().collect(),
                current: None,
                held: None,
            })),
        }
    }
}

struct ChainedSources {
    schema: SchemaRef,
    openers: VecDeque<SourceOpener>,
    current: Option<Box<dyn ThreadSource>>,
    held: Option<MemoryReservation>,
}

impl BatchOperator for ChainedSources {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.held = None;
        loop {
            if let Some(current) = self.current.as_mut() {
                if let Some(reserved) = current.next_batch()? {
                    self.held = reserved.memory;
                    return Ok(Some(reserved.batch));
                }
                self.current = None;
            }
            let Some(opener) = self.openers.pop_front() else {
                return Ok(None);
            };
            let source = opener()?;
            if source.schema() != &self.schema {
                return Err(error("parallel source schema differs between sources"));
            }
            self.current = Some(source);
        }
    }
}

/// Which thread a row belongs to when every thread sees every batch:
/// the thread its encoded key hashes to. Independent of the exchange's
/// partitioning of the same keys (another hash) and of any spill
/// partitioning nested inside (another salt).
pub struct ThreadSelector {
    hasher: ahash::RandomState,
    workers: usize,
}

impl ThreadSelector {
    pub fn new(workers: usize) -> Self {
        Self {
            hasher: ahash::RandomState::with_seeds(
                0x452A_F1AC_1B2D_9E33,
                0x9F6C_8B54_7D31_E0A7,
                0x3C0E_5B8D_D64A_1F29,
                0xB7E1_5162_8AED_2A6B,
            ),
            workers,
        }
    }

    #[inline]
    pub fn thread_of(&self, key: &[u8]) -> usize {
        (crate::exchange::mix(self.hasher.hash_one(key) ^ crate::exchange::THREAD_PARTITION_SALT)
            % self.workers as u64) as usize
    }
}

/// One operator run on several threads within a task: rows go to the
/// thread their key hashes to, so the threads hold disjoint keys and their
/// outputs union without a merge. The source stays on its calling thread;
/// only Arrow batches cross thread boundaries. Grouped partial aggregates
/// and DISTINCT run this way.
pub struct ParallelPartials {
    source: Option<Box<dyn BatchOperator>>,
    /// Sources read on threads of their own; taken when the operator starts.
    openers: Vec<SourceOpener>,
    source_schema: SchemaRef,
    schema: SchemaRef,
    dispatch: Dispatch,
    operator: ThreadOperator,
    pool: QueryMemoryPool,
    workers: usize,
    /// Each thread's operator spills through this when set: the query's
    /// shared spill budget and its partition count, decided once here so
    /// every thread takes the same path.
    spill: Option<(SpillManager, usize)>,
    stopped: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    /// The threads reading sources of their own.
    pumps: Vec<JoinHandle<()>>,
    /// One input channel per thread; empty once the source is drained.
    senders: Vec<SyncSender<QueuedBatch>>,
    partitioner: Option<HashPartitioner>,
    input_queue: Option<Arc<QueueBudget>>,
    round_robin: usize,
    /// Parts of the current source batch not yet handed to their thread.
    pending: VecDeque<(usize, QueuedBatch)>,
    /// Outputs taken while a thread's input was full: an operator that
    /// streams (DISTINCT) fills the output queue before its input is
    /// drained, so the pump must take from one to push to the other.
    ready: VecDeque<QueuedBatch>,
    output: Option<Receiver<Result<QueuedBatch>>>,
    current: Option<Held>,
    failed: bool,
}

/// How rows reach the threads.
enum Dispatch {
    /// Rows go to the thread their key hashes to, so the threads hold
    /// disjoint keys; unkeyed, slices round-robin. Keys that are all
    /// dictionary-encoded hold a handful of values, cheaper to fold on
    /// every thread than to hash-partition every row: with
    /// `fold_low_cardinality` those slice round-robin too. Only an
    /// operator whose outputs merge (a partial aggregate) may take that;
    /// DISTINCT must partition.
    Keyed {
        keys: Vec<String>,
        fold_low_cardinality: bool,
    },
    /// Every thread sees every batch and keeps its own rows: nothing is
    /// hashed or copied on the way, and sources read on their own threads
    /// feed the threads straight.
    Broadcast,
}

impl ParallelPartials {
    /// Grouped partial aggregation: every thread produces the typed partial
    /// batch the final merger takes.
    pub fn new(
        source: Box<dyn BatchOperator>,
        groups: Vec<String>,
        aggregates: Vec<AggExpr>,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        let types = aggregate_output_types(&aggregates, source.schema())?;
        let key_types = groups
            .iter()
            .map(|name| {
                source
                    .schema()
                    .field_with_name(name)
                    .map(|f| exchanged_group_key_type(f.data_type()))
                    .map_err(KaveonError::from)
            })
            .collect::<Result<Vec<_>>>()?;
        let schema = grouped_aggregate_states_to_schema_batch(&[], &key_types, &types)?.schema();
        // Validate source bindings before any threads are started.
        HashAggregate::new(
            Box::new(EmptyInput(source.schema().clone())),
            groups.clone(),
            aggregates.clone(),
        )?;
        let thread_groups = groups.clone();
        let operator: ThreadOperator = Arc::new(move |source, pool, context| {
            partial_aggregate_operator(
                source,
                thread_groups.clone(),
                aggregates.clone(),
                pool,
                context,
            )
        });
        Self::over(
            Sources::Here(source),
            schema,
            Dispatch::Keyed {
                keys: groups,
                fold_low_cardinality: true,
            },
            operator,
            pool,
            workers,
        )
    }

    /// DISTINCT over `columns`: every thread deduplicates the rows whose
    /// values hash to it, so the union of the threads' outputs is distinct.
    /// Columns that are all dictionary-encoded hold a handful of values:
    /// hashing every row to a thread would cost more than the whole
    /// DISTINCT, so the threads take slices round-robin and a serial
    /// DISTINCT over their small union settles the duplicates.
    pub fn distinct(
        source: Box<dyn BatchOperator>,
        columns: Vec<String>,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Box<dyn BatchOperator>> {
        if columns.is_empty() {
            return Err(error("parallel DISTINCT needs at least one column"));
        }
        for column in &columns {
            source
                .schema()
                .field_with_name(column)
                .map_err(|_| error(&format!("DISTINCT column '{column}' is not in the input")))?;
        }
        let schema = source.schema().clone();
        let low_cardinality = columns.iter().all(|name| {
            schema
                .field_with_name(name)
                .is_ok_and(|field| matches!(field.data_type(), DataType::Dictionary(_, _)))
        });
        let operator: ThreadOperator = Arc::new(move |source, pool, _| {
            Ok(Box::new(
                DistinctOperator::new(source).with_memory(pool.operator("parallel-distinct")?),
            ) as Box<dyn BatchOperator>)
        });
        let account = pool.operator("distinct-union")?;
        let parallel = Self::over(
            Sources::Here(source),
            schema,
            Dispatch::Keyed {
                keys: columns,
                fold_low_cardinality: low_cardinality,
            },
            operator,
            pool,
            workers,
        )?;
        Ok(if low_cardinality {
            Box::new(DistinctOperator::new(Box::new(parallel)).with_memory(account))
        } else {
            Box::new(parallel)
        })
    }

    /// Any operator whose input can be split by the hash of `keys` and
    /// whose outputs union without a merge, the rows hash-partitioned to
    /// the threads on the calling thread.
    pub fn partitioned(
        source: Box<dyn BatchOperator>,
        schema: SchemaRef,
        keys: Vec<String>,
        operator: ThreadOperator,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        Self::over(
            Sources::Here(source),
            schema,
            Dispatch::Keyed {
                keys,
                fold_low_cardinality: false,
            },
            operator,
            pool,
            workers,
        )
    }

    /// Any operator whose threads can each pick their own rows out of
    /// every batch (`ThreadSelector` over the row's key, with the thread's
    /// `ThreadContext::index`) and whose outputs union without a merge —
    /// the final aggregate over partial rows, for one. Every batch goes
    /// to every thread as it is: nothing is hashed or copied on the way,
    /// and sources of their own threads feed the threads straight.
    pub fn broadcast(
        sources: Sources,
        schema: SchemaRef,
        operator: ThreadOperator,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        Self::over(
            sources,
            schema,
            Dispatch::Broadcast,
            operator,
            pool,
            workers,
        )
    }

    fn over(
        sources: Sources,
        schema: SchemaRef,
        dispatch: Dispatch,
        operator: ThreadOperator,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        if !(1..=MAX_WORKERS).contains(&workers) {
            return Err(error("parallel worker count must be between 1 and 16"));
        }
        let spill = spill_from_environment(&pool)?;
        let source_schema = Arc::clone(sources.schema());
        let (source, openers) = match sources {
            Sources::Here(source) => (Some(source), Vec::new()),
            Sources::Threads { openers, .. } => {
                if openers.is_empty() {
                    return Err(error("parallel operator needs at least one source"));
                }
                (None, openers)
            }
        };
        Ok(Self {
            source,
            openers,
            source_schema,
            schema,
            dispatch,
            operator,
            pool,
            workers,
            spill,
            stopped: Arc::new(AtomicBool::new(false)),
            handles: vec![],
            pumps: vec![],
            senders: Vec::new(),
            partitioner: None,
            input_queue: None,
            round_robin: 0,
            pending: VecDeque::new(),
            ready: VecDeque::new(),
            output: None,
            current: None,
            failed: false,
        })
    }

    /// Every thread's operator spills through `spill` with `partitions`
    /// hash partitions, whatever the environment says.
    pub fn with_spill(mut self, spill: SpillManager, partitions: usize) -> Self {
        self.spill = Some((spill, partitions));
        self
    }

    fn start(&mut self) -> Result<()> {
        if self.source.is_none() && self.openers.is_empty() {
            return Err(error("parallel source already consumed"));
        }
        let (output_tx, output_rx) = mpsc::sync_channel(self.workers * 2);
        self.output = Some(output_rx);
        // Sources on threads of their own ask the operator's threads for
        // memory when the budget refuses them a batch.
        let pressure = (!self.openers.is_empty()).then(|| Pressure::new(self.workers));
        let mut senders = Vec::with_capacity(self.workers);
        for index in 0..self.workers {
            let (sender, receiver) = mpsc::sync_channel(2);
            senders.push(sender);
            let schema = Arc::clone(&self.source_schema);
            let operator = self.operator.clone();
            let pool = self.pool.clone();
            let context = ThreadContext {
                index,
                workers: self.workers,
                spill: self.spill.clone(),
                pressure: pressure.clone(),
            };
            let stopped = self.stopped.clone();
            let output = output_tx.clone();
            self.handles.push(
                thread::Builder::new()
                    .name(format!("kaveon-parallel-{index}"))
                    .spawn(move || {
                        let result = catch_worker_failure(|| {
                            let source = Box::new(ChannelInput {
                                schema,
                                receiver,
                                stopped: stopped.clone(),
                                pressure: context.pressure.clone().map(|p| (p, index)),
                                current: None,
                            });
                            run_worker(source, &operator, &pool, &context, &stopped, &output)
                        });
                        // However the thread ended, no request waits on it.
                        if let Some(pressure) = &context.pressure {
                            pressure.leave(index);
                        }
                        if let Err(err) = result {
                            let _ = output.send(Err(err));
                            stopped.store(true, Ordering::Release);
                        }
                    })
                    .map_err(|e| error(&format!("cannot spawn aggregate worker: {e}")))?,
            );
        }
        let queue = Arc::new(QueueBudget::new(
            self.pool.operator("parallel-input-queue")?,
        ));
        self.input_queue = Some(Arc::clone(&queue));
        // Keyed: rows go to the thread their key hashes to, so the threads
        // hold disjoint keys. Unkeyed, or keyed only by dictionary columns
        // where folding is allowed (a handful of groups, cheaper to fold N
        // times than to hash-partition every row): slices round-robin.
        self.partitioner = match &self.dispatch {
            Dispatch::Keyed {
                keys,
                fold_low_cardinality,
            } => {
                let low_cardinality_keys = *fold_low_cardinality
                    && keys.iter().all(|name| {
                        self.source_schema.field_with_name(name).is_ok_and(|field| {
                            matches!(field.data_type(), DataType::Dictionary(_, _))
                        })
                    });
                if keys.is_empty() || self.workers == 1 || low_cardinality_keys {
                    None
                } else {
                    Some(HashPartitioner::try_new_salted(
                        &self.source_schema,
                        keys,
                        self.workers,
                        crate::exchange::THREAD_PARTITION_SALT,
                    )?)
                }
            }
            Dispatch::Broadcast => None,
        };
        // Sources of their own threads: each reads its source and hands
        // every batch to every thread; the threads see the end of input
        // when the last of them has dropped its senders. Nothing else
        // holds a sender, so the calling thread's go now.
        let openers = std::mem::take(&mut self.openers);
        let pumps = openers.len();
        for (index, opener) in openers.into_iter().enumerate() {
            let senders = senders.clone();
            let schema = Arc::clone(&self.source_schema);
            let queue = Arc::clone(&queue);
            let pool = self.pool.clone();
            let stopped = self.stopped.clone();
            let output = output_tx.clone();
            let pressure = pressure.clone().expect("pumps have a pressure state");
            self.pumps.push(
                thread::Builder::new()
                    .name(format!("kaveon-parallel-source-{index}"))
                    .spawn(move || {
                        let result = catch_worker_failure(|| {
                            run_pump(
                                opener, &schema, &senders, &queue, pumps, &pool, &stopped,
                                &pressure,
                            )
                        });
                        drop(senders);
                        if let Err(err) = result {
                            let _ = output.send(Err(err));
                            stopped.store(true, Ordering::Release);
                        }
                    })
                    .map_err(|e| error(&format!("cannot spawn source thread: {e}")))?,
            );
        }
        drop(output_tx);
        self.senders = if pumps > 0 { Vec::new() } else { senders };
        Ok(())
    }

    /// Move one source batch to the threads. Returns false once the source
    /// is drained and the threads' inputs are closed. A full input queue is
    /// never waited on blindly: outputs are taken meanwhile, so a thread
    /// blocked on a full output queue is unblocked by the same loop.
    fn pump(&mut self) -> Result<bool> {
        let queue = self
            .input_queue
            .clone()
            .ok_or_else(|| error("parallel operator not started"))?;
        let account = queue.account.clone();
        if self.pending.is_empty() {
            let Some(source) = self.source.as_mut() else {
                return Ok(false);
            };
            let Some(batch) = source.next_batch()? else {
                self.source = None;
                self.senders.clear();
                // The queues' memory goes back with the last part the
                // threads take from them.
                self.input_queue = None;
                return Ok(false);
            };
            if batch.schema() != *source.schema() {
                return Err(error(
                    "parallel source batch does not match declared schema",
                ));
            }
            account.check_cancelled()?;
            let batch_bytes = occupied_bytes(&batch)?;
            queue.ensure_for(batch_bytes)?;
            match (&self.dispatch, &self.partitioner) {
                (Dispatch::Broadcast, _) => {
                    let memory = Arc::new(queue.charge(batch_bytes)?);
                    for worker in 0..self.workers {
                        self.pending.push_back((
                            worker,
                            QueuedBatch {
                                batch: batch.clone(),
                                _memory: Held::Charged {
                                    _charge: Arc::clone(&memory),
                                },
                            },
                        ));
                    }
                }
                (Dispatch::Keyed { .. }, Some(partitioner)) => {
                    for (worker, part) in partitioner.partition(&batch)?.into_iter().enumerate() {
                        if part.num_rows() == 0 {
                            continue;
                        }
                        let memory = Arc::new(queue.charge(occupied_bytes(&part)?)?);
                        self.pending.push_back((
                            worker,
                            QueuedBatch {
                                batch: part,
                                _memory: Held::Charged { _charge: memory },
                            },
                        ));
                    }
                }
                (Dispatch::Keyed { .. }, None) => {
                    let memory = Arc::new(queue.charge(batch_bytes)?);
                    for offset in (0..batch.num_rows()).step_by(8192) {
                        let slice = batch.slice(offset, 8192.min(batch.num_rows() - offset));
                        self.pending.push_back((
                            self.round_robin % self.workers,
                            QueuedBatch {
                                batch: slice,
                                _memory: Held::Charged {
                                    _charge: memory.clone(),
                                },
                            },
                        ));
                        self.round_robin += 1;
                    }
                }
            }
        }
        while let Some((worker, queued)) = self.pending.pop_front() {
            account.check_cancelled()?;
            if self.stopped.load(Ordering::Acquire) {
                return Err(stopped_error());
            }
            match self.senders[worker].try_send(queued) {
                Ok(()) => {}
                Err(TrySendError::Disconnected(_)) => {
                    return Err(error(DISCONNECTED));
                }
                Err(TrySendError::Full(returned)) => {
                    self.pending.push_front((worker, returned));
                    if !self.take_ready()? {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
        Ok(true)
    }

    /// Everything the threads have produced so far, without waiting.
    fn take_ready(&mut self) -> Result<bool> {
        let Some(output) = &self.output else {
            return Ok(false);
        };
        let mut taken = false;
        loop {
            match output.try_recv() {
                Ok(Ok(queued)) => {
                    self.ready.push_back(queued);
                    taken = true;
                }
                Ok(Err(err)) => return Err(err),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {
                    return Ok(taken);
                }
            }
        }
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.senders.clear();
        self.pending.clear();
        self.ready.clear();
        self.output = None;
        for handle in self.pumps.drain(..).chain(self.handles.drain(..)) {
            let _ = handle.join();
        }
        self.current = None;
    }
}
impl BatchOperator for ParallelPartials {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.failed {
            return Ok(None);
        }
        self.current = None;
        let result = (|| {
            if (self.source.is_some() || !self.openers.is_empty()) && self.output.is_none() {
                self.start()?;
            }
            let account = self.pool.operator("parallel-output-queue")?;
            loop {
                account.check_cancelled()?;
                if let Some(queued) = self.ready.pop_front() {
                    self.current = Some(queued._memory);
                    return Ok(Some(queued.batch));
                }
                // Feed the threads while the source lasts; outputs taken
                // along the way come back through `ready`.
                if self.source.is_some() {
                    self.pump()?;
                    self.take_ready()?;
                    continue;
                }
                // Sources on their own threads: once they are all read,
                // the queues' memory goes back with the last part the
                // threads take.
                if self.input_queue.is_some()
                    && !self.pumps.is_empty()
                    && self.pumps.iter().all(JoinHandle::is_finished)
                {
                    self.input_queue = None;
                }
                let Some(output) = &self.output else {
                    return Ok(None);
                };
                match output.recv_timeout(Duration::from_millis(20)) {
                    Ok(Ok(queued)) => {
                        self.current = Some(queued._memory);
                        return Ok(Some(queued.batch));
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        self.stop();
                        return Ok(None);
                    }
                }
            }
        })();
        let result = result.map_err(|error| self.thread_failure(error));
        if result.is_err() {
            self.failed = true;
            self.stop();
        }
        result
    }
}
impl ParallelPartials {
    /// The error behind `error` when a thread failed: a thread reports
    /// its error on the output channel after dropping its input, so the
    /// pump can see the input disconnect first. Stop the threads, drain
    /// the outputs while they finish — a thread reporting into a full
    /// channel must not be waited on blindly — and take the first error
    /// they reported, or `error` when there is none.
    fn thread_failure(&mut self, error: KaveonError) -> KaveonError {
        self.stopped.store(true, Ordering::Release);
        self.senders.clear();
        self.pending.clear();
        // The first error that is not a consequence of the stop or of a
        // thread's exit — a failing thread drops its input before it
        // reports, so the pump can see the disconnect, and its siblings
        // report the stop, ahead of the cause.
        let mut reported = error;
        let take = |failure: KaveonError, reported: &mut KaveonError| {
            if is_consequence(reported) && !is_consequence(&failure) {
                *reported = failure;
            }
        };
        while let Some(output) = &self.output {
            for message in output.try_iter() {
                if let Err(failure) = message {
                    take(failure, &mut reported);
                }
            }
            if self
                .handles
                .iter()
                .chain(&self.pumps)
                .all(JoinHandle::is_finished)
            {
                // What a thread reported between the drain and its end.
                for message in output.try_iter() {
                    if let Err(failure) = message {
                        take(failure, &mut reported);
                    }
                }
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        reported
    }
}
impl Drop for ParallelPartials {
    fn drop(&mut self) {
        self.stop();
    }
}
fn send_bounded<T>(
    sender: &SyncSender<T>,
    mut value: T,
    stopped: &AtomicBool,
    pool: &QueryMemoryPool,
) -> Result<()> {
    let account = pool.operator("parallel-channel")?;
    loop {
        account.check_cancelled()?;
        if stopped.load(Ordering::Acquire) {
            return Err(stopped_error());
        }
        match sender.try_send(value) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err(error(DISCONNECTED));
            }
            Err(TrySendError::Full(returned)) => {
                value = returned;
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
/// The bytes a batch's rows occupy: its columns' data, not the capacity
/// of the buffers behind them — a batch decoded from one IPC message has
/// every column's buffers pointing at that whole message, and the
/// capacity would count it once per buffer. What a source reserves for a
/// decoded batch, and what the queue charges for one that came without a
/// reservation.
pub fn occupied_bytes(batch: &RecordBatch) -> Result<u64> {
    batch
        .columns()
        .iter()
        .map(|column| {
            column
                .to_data()
                .get_slice_memory_size()
                .map(|bytes| bytes as u64)
                .map_err(KaveonError::from)
        })
        .sum()
}

/// One source read on its own thread, every batch to every thread. A
/// batch the budget refuses the source is not the end: the threads are
/// asked for memory (`Pressure::request`) and the source is asked again
/// — it kept the batch — until the reservation is taken or no thread has
/// anything to give, when the refusal stands.
#[allow(clippy::too_many_arguments)]
fn run_pump(
    opener: SourceOpener,
    schema: &SchemaRef,
    senders: &[SyncSender<QueuedBatch>],
    queue: &Arc<QueueBudget>,
    pumps: usize,
    pool: &QueryMemoryPool,
    stopped: &AtomicBool,
    pressure: &Pressure,
) -> Result<()> {
    let mut source = opener()?;
    if source.schema() != schema {
        return Err(error("parallel source schema differs between sources"));
    }
    let account = pool.operator("parallel-source")?;
    loop {
        // What the threads had given up before this attempt: a refusal
        // that predates a release is not the last word.
        let since = pressure.given_up();
        let reserved = match source.next_batch() {
            Ok(Some(reserved)) => reserved,
            Ok(None) => return Ok(()),
            Err(KaveonError::MemoryLimit(message)) => {
                match pressure.request(since, &account, stopped)? {
                    Relief::Released => continue,
                    Relief::Nothing => return Err(KaveonError::MemoryLimit(message)),
                }
            }
            Err(error) => return Err(error),
        };
        let batch = reserved.batch;
        if batch.schema() != *schema {
            return Err(error(
                "parallel source batch does not match declared schema",
            ));
        }
        if stopped.load(Ordering::Acquire) {
            return Err(stopped_error());
        }
        let held = match reserved.memory {
            // The source's reservation crosses with the batch: the queues
            // hold it until the last thread is done with the batch.
            Some(reservation) => Held::Reserved {
                _guard: Arc::new(reservation),
            },
            None => {
                let bytes = occupied_bytes(&batch)?;
                // The queues hold this many batches from every source at once.
                queue.ensure_for(bytes.saturating_mul(pumps as u64))?;
                Held::Charged {
                    _charge: Arc::new(queue.charge_waiting(bytes, stopped)?),
                }
            }
        };
        for sender in senders {
            send_bounded(
                sender,
                QueuedBatch {
                    batch: batch.clone(),
                    _memory: held.clone(),
                },
                stopped,
                pool,
            )?;
        }
    }
}

fn catch_worker_failure(work: impl FnOnce() -> Result<()>) -> Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err(error("parallel aggregate worker panicked")))
}
fn run_worker(
    source: Box<dyn BatchOperator>,
    operator: &ThreadOperator,
    pool: &QueryMemoryPool,
    context: &ThreadContext,
    stopped: &AtomicBool,
    output: &SyncSender<Result<QueuedBatch>>,
) -> Result<()> {
    let account = pool.operator("parallel-output")?;
    let mut operator = operator(source, pool, context)?;
    // The operator has taken a responder's ticket by now or never will:
    // a request does not wait on this thread past this point unless it
    // answers.
    if let Some(pressure) = &context.pressure {
        pressure.settle(context.index);
    }
    while let Some(batch) = operator.next_batch()? {
        // The batch is held by the queue; whatever it came from is not.
        let memory = Arc::new(account.reserve(batch.get_array_memory_size() as u64 + 8192)?);
        send_bounded(
            output,
            Ok(QueuedBatch {
                batch,
                _memory: Held::Reserved { _guard: memory },
            }),
            stopped,
            pool,
        )?;
    }
    Ok(())
}

/// One thread's partial aggregate: spill-capable with its share of the
/// adaptive buffer when the query spills, the in-memory columnar partial
/// otherwise.
fn partial_aggregate_operator(
    source: Box<dyn BatchOperator>,
    groups: Vec<String>,
    aggregates: Vec<AggExpr>,
    pool: &QueryMemoryPool,
    context: &ThreadContext,
) -> Result<Box<dyn BatchOperator>> {
    let account = pool.operator("parallel-partial-aggregate")?;
    if let Some((spill, count)) = context.spill.clone() {
        // The threads together buffer what one serial aggregate would.
        return Ok(Box::new(
            PartitionedHashAggregate::new_partial(
                source,
                groups,
                aggregates,
                account.clone(),
                spill,
                count,
            )?
            .with_reserved_input()
            .with_budget_share(context.workers)?,
        ));
    }
    Ok(Box::new(
        FlushingPartialAggregate::new(source, groups, aggregates, account)?
            .with_reserved_input()
            .with_budget_share(context.workers),
    ))
}

/// A point every thread of one parallel operator reaches before any goes
/// on: the final merge's threads emit only once all have finished
/// merging, so no thread's output competes for the budget with another's
/// growing table. A thread that fails or is dropped counts as arrived, so
/// the others are never left waiting for it; a cancelled query leaves.
pub struct Rendezvous {
    parties: usize,
    arrived: Mutex<usize>,
    all_arrived: std::sync::Condvar,
}

impl Rendezvous {
    pub fn new(parties: usize) -> Arc<Self> {
        Arc::new(Self {
            parties,
            arrived: Mutex::new(0),
            all_arrived: std::sync::Condvar::new(),
        })
    }

    /// One party's place at the rendezvous.
    pub fn ticket(self: &Arc<Self>) -> RendezvousTicket {
        RendezvousTicket {
            rendezvous: Arc::clone(self),
            arrived: false,
        }
    }

    fn arrive(&self) {
        if let Ok(mut arrived) = self.arrived.lock() {
            *arrived += 1;
            if *arrived >= self.parties {
                self.all_arrived.notify_all();
            }
        }
    }
}

pub struct RendezvousTicket {
    rendezvous: Arc<Rendezvous>,
    arrived: bool,
}

impl RendezvousTicket {
    /// Arrive, and wait for the others.
    pub fn wait(mut self, memory: &OperatorMemoryAccount) -> Result<()> {
        self.arrived = true;
        self.rendezvous.arrive();
        let mut arrived = self
            .rendezvous
            .arrived
            .lock()
            .map_err(|_| error("parallel rendezvous poisoned"))?;
        while *arrived < self.rendezvous.parties {
            memory.check_cancelled()?;
            arrived = self
                .rendezvous
                .all_arrived
                .wait_timeout(arrived, Duration::from_millis(20))
                .map_err(|_| error("parallel rendezvous poisoned"))?
                .0;
        }
        Ok(())
    }
}

impl Drop for RendezvousTicket {
    fn drop(&mut self) {
        if !self.arrived {
            self.arrived = true;
            self.rendezvous.arrive();
        }
    }
}

// --- Memory pressure from the source threads ----------------------------------
// A source read on its own thread reserves each batch it decodes. When the
// budget refuses one, the memory is with the operator's threads — the final
// merge's tables, which can go to the disk. Rather than fail the task, the
// source raises a request here and waits; the threads answer between
// batches, and the source tries again once one of them has given memory
// up, or gives up once none has anything to give.
//
// The rule, kept simple:
// - One request is open at a time. A source refused while one is open
//   waits on the same request; whoever retries first takes what was freed,
//   and the other raises the next request if it is refused again.
// - Every thread answers a request once, in the order the threads reach
//   it: a thread whose operator holds something to give up claims the
//   request and gives it up (the final merge spills its live table); the
//   first to claim is the only one to — the source retries after one
//   spill, and a second refusal is a new request. A thread with nothing
//   to give declines.
// - The request resolves `Released` as soon as any thread has given
//   memory up — the claimer, or a thread that spilled for a refusal of
//   its own meanwhile — and `Nothing` when every thread has declined and
//   none gave anything up since the source's attempt, which is the
//   refusal made final. (Threads sharing one budget reach their refusals
//   together: a thread that spilled on its own and then declines has
//   still freed what the source needs.)
// - A thread that never takes a responder's ticket (its operator has no
//   memory to give up), or that has left (finished, failed, dropped), is
//   never waited on. A request raised before a thread's operator exists
//   waits for it to take a ticket or not — that is decided as the
//   operator is constructed, never later.
// - A source waits on a condvar, holding no lock; it leaves on stop or
//   cancellation. A thread blocked on its input wakes to answer
//   (`ChannelInput`), so no request waits on a thread that has nothing to
//   do.

/// What a request came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relief {
    /// A thread gave memory up: try the reservation again.
    Released,
    /// No thread has anything to give: the refusal is final.
    Nothing,
}

/// The pressure state shared by the sources and threads of one parallel
/// operator.
pub struct Pressure {
    state: Mutex<PressureState>,
    changed: Condvar,
    /// The open request's generation, 0 when none: what a thread checks
    /// between batches without the lock.
    open: AtomicU64,
    /// Per thread: the generation it last answered.
    answered: Vec<AtomicU64>,
    /// Per thread: whether it holds a responder's ticket.
    responding: Vec<AtomicBool>,
    /// Times a thread has given memory up, on a request or on a refusal
    /// of its own: what a source reads before an attempt, so a refusal
    /// can be told from one made before a thread gave memory up.
    given_up: AtomicU64,
}

struct PressureState {
    /// Generations raised so far.
    generation: u64,
    request: Option<PressureRequest>,
    roles: Vec<ThreadRole>,
    /// The last resolved request, for the sources waiting on it.
    last: Option<(u64, Relief)>,
    stats: PressureStats,
}

struct PressureRequest {
    generation: u64,
    /// The thread giving memory up for it, once one has claimed it.
    claimer: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThreadRole {
    /// The operator is not constructed yet.
    Unknown,
    /// The operator answers requests.
    Responder,
    /// Nothing to ask of it.
    Absent,
}

/// What the requests came to, for the record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PressureStats {
    pub requests: u64,
    pub released: u64,
    pub nothing: u64,
}

impl Pressure {
    pub fn new(threads: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PressureState {
                generation: 0,
                request: None,
                roles: vec![ThreadRole::Unknown; threads],
                last: None,
                stats: PressureStats::default(),
            }),
            changed: Condvar::new(),
            open: AtomicU64::new(0),
            answered: (0..threads).map(|_| AtomicU64::new(0)).collect(),
            responding: (0..threads).map(|_| AtomicBool::new(false)).collect(),
            given_up: AtomicU64::new(0),
        })
    }

    /// Times the threads have given memory up so far: read before an
    /// attempt, passed to `request` with its refusal.
    pub fn given_up(&self) -> u64 {
        self.given_up.load(Ordering::Acquire)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, PressureState>> {
        self.state
            .lock()
            .map_err(|_| error("parallel pressure state poisoned"))
    }

    pub fn stats(&self) -> PressureStats {
        self.state
            .lock()
            .map_or(PressureStats::default(), |state| state.stats)
    }

    /// Thread `index`'s place as a responder: its operator answers
    /// requests, with this.
    pub fn responder(self: &Arc<Self>, index: usize) -> PressureTicket {
        if let Ok(mut state) = self.state.lock() {
            state.roles[index] = ThreadRole::Responder;
            self.responding[index].store(true, Ordering::Release);
        }
        PressureTicket {
            pressure: Arc::clone(self),
            index,
        }
    }

    /// Thread `index`'s operator exists: if it took no ticket, it never
    /// will.
    fn settle(&self, index: usize) {
        if let Ok(mut state) = self.state.lock()
            && state.roles[index] == ThreadRole::Unknown
        {
            state.roles[index] = ThreadRole::Absent;
            self.resolve_if_declined(&mut state);
        }
    }

    /// Thread `index` is gone, or its operator has nothing more to give.
    fn leave(&self, index: usize) {
        self.responding[index].store(false, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.roles[index] = ThreadRole::Absent;
            if let Some(request) = &mut state.request
                && request.claimer == Some(index)
            {
                // Whatever it held is released with it, or was not for
                // lack of memory: either way the source asks again.
                self.resolve(&mut state, Relief::Released);
                return;
            }
            self.resolve_if_declined(&mut state);
        }
    }

    /// Whether the open request waits on thread `index`'s answer.
    fn awaits(&self, index: usize) -> bool {
        let generation = self.open.load(Ordering::Acquire);
        generation != 0
            && self.responding[index].load(Ordering::Acquire)
            && self.answered[index].load(Ordering::Acquire) != generation
    }

    /// Thread `index` answers the open request with something to give
    /// up: true when it is the one to, false when another claimed first
    /// (nothing more is asked of it for this request).
    fn claim(&self, index: usize) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some(request) = &mut state.request else {
            return false;
        };
        if self.answered[index].load(Ordering::Acquire) == request.generation {
            return false;
        }
        self.answered[index].store(request.generation, Ordering::Release);
        if request.claimer.is_some() {
            return false;
        }
        request.claimer = Some(index);
        true
    }

    /// Thread `index` has nothing to give up for the open request.
    fn decline(&self, index: usize) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(request) = &state.request {
                self.answered[index].store(request.generation, Ordering::Release);
            }
            self.resolve_if_declined(&mut state);
        }
    }

    /// Thread `index` has given memory up — for the request it claimed,
    /// or for a refusal of its own. Either frees what a source waits for:
    /// the open request, if any, is resolved `Released`, and the thread
    /// has answered it.
    fn released(&self, index: usize) {
        if let Ok(mut state) = self.state.lock() {
            self.given_up.fetch_add(1, Ordering::AcqRel);
            if let Some(request) = &state.request {
                self.answered[index].store(request.generation, Ordering::Release);
                self.resolve(&mut state, Relief::Released);
            }
        }
    }

    /// An unclaimed request every thread that could answer has declined
    /// is final.
    fn resolve_if_declined(&self, state: &mut PressureState) {
        let Some(request) = &state.request else {
            return;
        };
        if request.claimer.is_some() {
            return;
        }
        let generation = request.generation;
        let all_declined = state
            .roles
            .iter()
            .enumerate()
            .all(|(index, role)| match role {
                ThreadRole::Unknown => false,
                ThreadRole::Absent => true,
                ThreadRole::Responder => self.answered[index].load(Ordering::Acquire) == generation,
            });
        if all_declined {
            self.resolve(state, Relief::Nothing);
        }
    }

    fn resolve(&self, state: &mut PressureState, relief: Relief) {
        let Some(request) = state.request.take() else {
            return;
        };
        match relief {
            Relief::Released => state.stats.released += 1,
            Relief::Nothing => state.stats.nothing += 1,
        }
        state.last = Some((request.generation, relief));
        self.open.store(0, Ordering::Release);
        self.changed.notify_all();
    }

    /// A source's reservation was refused: ask the threads for memory and
    /// wait for the answer. `since` is `given_up()` as read before the
    /// refused attempt: a `Nothing` from the threads is `Released` when
    /// one of them has given memory up since, the attempt having come
    /// first. Leaves with the stop or the query's cancellation.
    pub fn request(
        &self,
        since: u64,
        account: &OperatorMemoryAccount,
        stopped: &AtomicBool,
    ) -> Result<Relief> {
        let mut state = self.lock()?;
        let generation = match &state.request {
            Some(request) => request.generation,
            None => {
                state.generation += 1;
                let generation = state.generation;
                state.stats.requests += 1;
                state.request = Some(PressureRequest {
                    generation,
                    claimer: None,
                });
                self.open.store(generation, Ordering::Release);
                self.resolve_if_declined(&mut state);
                generation
            }
        };
        loop {
            if let Some((resolved, relief)) = state.last
                && resolved >= generation
            {
                // Mine, or a later one whose outcome replaced it: the
                // retry is the reservation's to decide either way.
                return Ok(
                    if resolved == generation && self.given_up.load(Ordering::Acquire) == since {
                        relief
                    } else {
                        Relief::Released
                    },
                );
            }
            if stopped.load(Ordering::Acquire) {
                return Err(stopped_error());
            }
            account.check_cancelled()?;
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .map_err(|_| error("parallel pressure state poisoned"))?
                .0;
        }
    }
}

/// A thread's place as a responder to pressure requests. Dropped, the
/// thread has nothing more to give: no request waits on it.
pub struct PressureTicket {
    pressure: Arc<Pressure>,
    index: usize,
}

impl PressureTicket {
    /// Whether a request waits on this thread's answer.
    pub fn pending(&self) -> bool {
        self.pressure.awaits(self.index)
    }

    /// Answer the open request with something to give up: true when this
    /// thread is to give it up now (and report `released` after), false
    /// when another thread claimed the request first — or when no request
    /// is open any more.
    pub fn claim(&self) -> bool {
        self.pressure.claim(self.index)
    }

    /// Answer the open request with nothing to give up.
    pub fn decline(&self) {
        self.pressure.decline(self.index);
    }

    /// This thread has given memory up: what it claimed, or its table
    /// spilled for a refusal of its own. The open request, if any, is
    /// answered with it.
    pub fn released(&self) {
        self.pressure.released(self.index);
    }
}

impl Drop for PressureTicket {
    fn drop(&mut self) {
        self.pressure.leave(self.index);
    }
}

pub type Finalizer = Box<dyn FnOnce(Box<dyn BatchOperator>) -> Result<Box<dyn BatchOperator>>>;
/// Defers partial execution/final merging until next_batch, preserving planner laziness.
pub struct LazyFinalAggregate {
    schema: SchemaRef,
    input: Option<Box<dyn BatchOperator>>,
    finalize: Option<Finalizer>,
    output: Option<Box<dyn BatchOperator>>,
}
impl LazyFinalAggregate {
    pub fn new(schema: SchemaRef, input: Box<dyn BatchOperator>, finalize: Finalizer) -> Self {
        Self {
            schema,
            input: Some(input),
            finalize: Some(finalize),
            output: None,
        }
    }
}
impl BatchOperator for LazyFinalAggregate {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if let Some(finalize) = self.finalize.take() {
            let output = finalize(
                self.input
                    .take()
                    .ok_or_else(|| error("missing partial input"))?,
            )?;
            if output.schema().fields().len() != self.schema.fields().len()
                || output
                    .schema()
                    .fields()
                    .iter()
                    .zip(self.schema.fields())
                    .any(|(actual, expected)| {
                        actual.name() != expected.name()
                            || actual.data_type() != expected.data_type()
                    })
            {
                return Err(error(
                    "parallel final aggregate field names or types differ from planned schema",
                ));
            }
            self.output = Some(output);
        }
        match &mut self.output {
            Some(output) => output
                .next_batch()?
                .map(|batch| {
                    // Partial state transports types but not source field nullability/metadata.
                    // Restore the planner schema while Arrow validates its column constraints.
                    RecordBatch::try_new(self.schema.clone(), batch.columns().to_vec())
                        .map_err(KaveonError::from)
                })
                .transpose(),
            None => Ok(None),
        }
    }
}
pub struct EmptyInput(pub SchemaRef);
impl BatchOperator for EmptyInput {
    fn schema(&self) -> &SchemaRef {
        &self.0
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        Ok(None)
    }
}
fn error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

/// The error a thread fails with because the operator was stopped — a
/// consequence of another failure, never the cause reported.
const STOPPED: &str = "parallel operator stopped";

fn stopped_error() -> KaveonError {
    KaveonError::Execution(STOPPED.into())
}

const DISCONNECTED: &str = "parallel operator channel disconnected";

/// An error that follows from another thread's failure rather than
/// causing it.
fn is_consequence(error: &KaveonError) -> bool {
    matches!(error, KaveonError::Execution(message) if message == STOPPED || message == DISCONNECTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{
        AggFunc, AggregateState, FinalAggregateValue, finalize_grouped_aggregate_states,
        grouped_aggregate_states_from_batches, merge_grouped_aggregate_states,
    };
    use arrow::{
        array::{
            Array, ArrayRef, Decimal128Array, Float64Array, Int32Array, Int64Array, StringArray,
            UInt64Array,
        },
        datatypes::{DataType, Field, Int32Type, Schema},
    };
    use std::collections::VecDeque;
    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }
    impl Input {
        fn one(batch: RecordBatch) -> Self {
            Self {
                schema: batch.schema(),
                batches: VecDeque::from([batch]),
            }
        }
    }
    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    #[test]
    fn parallel_partials_match_serial_typed_aggregates_and_empty_inputs() {
        let values = (0..25000)
            .map(|n| (n % 11 != 0).then_some((n % 37) as i64))
            .collect::<Vec<_>>();
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(values.clone())),
            Arc::new(Int32Array::from(
                values
                    .iter()
                    .map(|v| v.map(|n| n as i32))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                values
                    .iter()
                    .map(|v| v.map(|n| n as u64))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                values
                    .iter()
                    .map(|v| v.map(|n| n as f64))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(
                Decimal128Array::from(
                    values
                        .iter()
                        .map(|v| v.map(|n| n as i128))
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(30, 3)
                .unwrap(),
            ),
        ];
        for array in arrays {
            let batch = RecordBatch::try_from_iter(vec![
                (
                    "k",
                    Arc::new(Int64Array::from(
                        (0..25000)
                            .map(|n| (n % 5 != 0).then_some((n % 17) as i64))
                            .collect::<Vec<_>>(),
                    )) as ArrayRef,
                ),
                ("v", array),
            ])
            .unwrap();
            let expressions = vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "v"),
                AggExpr::new(AggFunc::Min, "v"),
                AggExpr::new(AggFunc::Max, "v"),
                AggExpr::new(AggFunc::Avg, "v"),
                AggExpr::new(AggFunc::Count, "v").distinct(),
                AggExpr::new(AggFunc::Sum, "v").distinct(),
                AggExpr::new(AggFunc::Avg, "v").distinct(),
            ];
            for groups in [vec![], vec!["k".into()]] {
                for batch in [batch.clone(), RecordBatch::new_empty(batch.schema())] {
                    let expected = HashAggregate::new(
                        Box::new(Input::one(batch.clone())),
                        groups.clone(),
                        expressions.clone(),
                    )
                    .unwrap()
                    .into_grouped_states()
                    .unwrap();
                    let expected = finalize_grouped_aggregate_states(
                        &merge_grouped_aggregate_states(expected).unwrap(),
                    )
                    .unwrap();
                    let pool = QueryMemoryPool::new("parallel-types", 32 * 1024 * 1024).unwrap();
                    let mut operator = ParallelPartials::new(
                        Box::new(Input::one(batch)),
                        groups.clone(),
                        expressions.clone(),
                        pool.clone(),
                        4,
                    )
                    .unwrap();
                    let mut actual = vec![];
                    while let Some(batch) = operator.next_batch().unwrap() {
                        actual.extend(grouped_aggregate_states_from_batches(&[batch]).unwrap());
                    }
                    let actual = finalize_grouped_aggregate_states(
                        &merge_grouped_aggregate_states(actual).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(actual.len(), expected.len());
                    for (actual, expected) in actual.iter().zip(expected) {
                        assert_eq!(actual.group_keys, expected.group_keys);
                        for (actual, expected) in actual.values.iter().zip(expected.values) {
                            match (actual, &expected) {
                                (
                                    FinalAggregateValue::Numeric(Some(a)),
                                    FinalAggregateValue::Numeric(Some(b)),
                                ) => assert!((a - b).abs() < 1e-8),
                                _ => assert_eq!(actual, &expected),
                            }
                        }
                    }
                    drop(operator);
                    assert_eq!(pool.snapshot().current_bytes, 0);
                }
            }
        }
    }
    #[test]
    fn parallel_distinct_matches_the_serial_operator_and_never_folds() {
        // Text, dictionary and integer columns with nulls across many more
        // batches than the queues hold — DISTINCT streams, so the threads
        // fill the output queue long before their input is drained and the
        // pump must take from one side to push the other. The threads'
        // outputs union to exactly the serial DISTINCT — including when
        // every column is dictionary-encoded, where a partial aggregate
        // would fold but DISTINCT must partition.
        let dictionary = |values: Vec<Option<&str>>| -> ArrayRef {
            let mut builder = arrow::array::StringDictionaryBuilder::<Int32Type>::new();
            for value in values {
                builder.append_option(value);
            }
            Arc::new(builder.finish())
        };
        let rows = 200_000usize;
        let make = |chunk: usize, dictionary_only: bool| {
            let range = chunk * 5000..(chunk + 1) * 5000;
            let country = range
                .clone()
                .map(|n| (n % 13 != 0).then(|| ["us", "de", "jp", "br"][n % 4]))
                .collect::<Vec<_>>();
            let surface = range
                .clone()
                .map(|n| (n % 7 != 0).then(|| ["a", "b", "c"][n % 3]))
                .collect::<Vec<_>>();
            let mut columns: Vec<(&str, ArrayRef)> = vec![
                ("country", dictionary(country)),
                ("surface", dictionary(surface)),
            ];
            if !dictionary_only {
                columns.push((
                    "bucket",
                    Arc::new(Int64Array::from_iter(
                        range.map(|n| (n % 11 != 0).then_some((n % 97) as i64)),
                    )),
                ));
            }
            RecordBatch::try_from_iter(columns).unwrap()
        };
        for dictionary_only in [false, true] {
            let batches = (0..rows / 5000)
                .map(|chunk| make(chunk, dictionary_only))
                .collect::<VecDeque<_>>();
            let schema = batches[0].schema();
            let columns = schema
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect::<Vec<_>>();
            let collect = |mut operator: Box<dyn BatchOperator>| {
                let mut rows = Vec::new();
                while let Some(batch) = operator.next_batch().unwrap() {
                    let batch = RecordBatch::try_new(
                        Arc::new(Schema::new(
                            batch
                                .schema()
                                .fields()
                                .iter()
                                .map(|f| Field::new(f.name(), DataType::Utf8, true))
                                .collect::<Vec<_>>(),
                        )),
                        batch
                            .columns()
                            .iter()
                            .map(|c| arrow::compute::cast(c, &DataType::Utf8).unwrap())
                            .collect(),
                    )
                    .unwrap();
                    for row in 0..batch.num_rows() {
                        rows.push(
                            batch
                                .columns()
                                .iter()
                                .map(|c| {
                                    let c = c.as_any().downcast_ref::<StringArray>().unwrap();
                                    (!c.is_null(row)).then(|| c.value(row).to_owned())
                                })
                                .collect::<Vec<_>>(),
                        );
                    }
                }
                rows.sort();
                rows
            };
            let serial = collect(Box::new(DistinctOperator::new(Box::new(Input {
                schema: schema.clone(),
                batches: batches.clone(),
            }))));
            let pool = QueryMemoryPool::new("parallel-distinct", 64 * 1024 * 1024).unwrap();
            let parallel = collect(
                ParallelPartials::distinct(
                    Box::new(Input { schema, batches }),
                    columns,
                    pool.clone(),
                    4,
                )
                .unwrap(),
            );
            assert_eq!(parallel, serial);
            assert!(serial.len() > 10);
            assert_eq!(pool.snapshot().current_bytes, 0);
        }
    }

    #[test]
    fn parallel_partials_flush_per_thread_under_a_tight_budget() {
        // Unique keys, more of them than a thread's share of the budget
        // holds: every thread flushes its groups in rounds inside its own
        // share, the union of the threads' partials is still one row per
        // key, and the disk is never touched — partial groups merge, so a
        // grouped partial has no reason to spill.
        let rows: usize = 1_200_000;
        let batch_rows = 8192;
        let batches = (0..rows.div_ceil(batch_rows))
            .map(|chunk| {
                let start = chunk * batch_rows;
                let end = rows.min(start + batch_rows);
                RecordBatch::try_from_iter(vec![
                    (
                        "k",
                        Arc::new(Int64Array::from_iter_values((start..end).map(|n| n as i64)))
                            as ArrayRef,
                    ),
                    (
                        "v",
                        Arc::new(Int64Array::from_iter_values(
                            (start..end).map(|n| (n % 1000) as i64),
                        )) as ArrayRef,
                    ),
                ])
                .unwrap()
            })
            .collect::<VecDeque<_>>();
        let schema = batches[0].schema();
        let spill = SpillManager::new(
            std::env::temp_dir().join("kaveon-parallel-spill-tests"),
            64 * 1024 * 1024,
        )
        .unwrap();
        let pool = QueryMemoryPool::new("parallel-spill", 64 * 1024 * 1024).unwrap();
        let mut operator = ParallelPartials::new(
            Box::new(Input { schema, batches }),
            vec!["k".into()],
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "v"),
            ],
            pool.clone(),
            4,
        )
        .unwrap()
        .with_spill(spill.clone(), 16);
        let mut groups = 0;
        let mut partial_batches = 0;
        while let Some(batch) = operator.next_batch().unwrap() {
            partial_batches += 1;
            for group in grouped_aggregate_states_from_batches(&[batch]).unwrap() {
                assert_eq!(group.states[0], AggregateState::Count(1));
                groups += 1;
            }
        }
        assert_eq!(groups, rows);
        assert!(
            partial_batches > 4,
            "every thread emits several flush rounds"
        );
        assert_eq!(spill.snapshot().runs_written, 0);
        assert!(pool.snapshot().peak_bytes <= 64 * 1024 * 1024);
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn sliced_buffers_share_one_reservation_and_drop_joins_workers() {
        let batch = RecordBatch::try_from_iter(vec![(
            "v",
            Arc::new(Int64Array::from(vec![1; 100000])) as ArrayRef,
        )])
        .unwrap();
        let pool = QueryMemoryPool::new("shared-slices", 3 * 1024 * 1024).unwrap();
        let mut operator = ParallelPartials::new(
            Box::new(Input::one(batch)),
            vec![],
            vec![AggExpr::new(AggFunc::Count, "*")],
            pool.clone(),
            4,
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_some());
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes < 3 * 1024 * 1024);
    }
    #[test]
    fn cancellation_after_dispatch_stops_active_workers() {
        struct CancellingInput {
            schema: SchemaRef,
            batch: Option<RecordBatch>,
            cancelled: Arc<AtomicBool>,
        }
        impl BatchOperator for CancellingInput {
            fn schema(&self) -> &SchemaRef {
                &self.schema
            }
            fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
                if let Some(batch) = self.batch.take() {
                    Ok(Some(batch))
                } else {
                    self.cancelled.store(true, Ordering::Release);
                    Ok(None)
                }
            }
        }
        let batch = RecordBatch::try_from_iter(vec![(
            "v",
            Arc::new(Int64Array::from(vec![1; 100000])) as ArrayRef,
        )])
        .unwrap();
        let pool = QueryMemoryPool::new("active-cancel", 4 * 1024 * 1024).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        pool.set_cancellation_probe(move || signal.load(Ordering::Acquire))
            .unwrap();
        let source = CancellingInput {
            schema: batch.schema(),
            batch: Some(batch),
            cancelled,
        };
        let mut operator = ParallelPartials::new(
            Box::new(source),
            vec![],
            vec![AggExpr::new(AggFunc::Sum, "v")],
            pool.clone(),
            4,
        )
        .unwrap();
        assert!(operator.next_batch().is_err());
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn worker_panic_boundary_releases_live_reservations() {
        let pool = QueryMemoryPool::new("panic-boundary", 1024).unwrap();
        let result = catch_worker_failure(|| {
            let _guard = pool.operator("worker")?.reserve(512)?;
            panic!("injected operator panic");
        });
        assert!(result.unwrap_err().to_string().contains("worker panicked"));
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn budget_failure_cancellation_and_schema_mismatch_release_all_resources() {
        for mode in 0..3 {
            let batch = RecordBatch::try_from_iter(vec![(
                "v",
                Arc::new(Int64Array::from(vec![1; 20000])) as ArrayRef,
            )])
            .unwrap();
            let pool = QueryMemoryPool::new(
                "parallel-failure",
                if mode == 0 { 1024 } else { 4 * 1024 * 1024 },
            )
            .unwrap();
            if mode == 1 {
                pool.set_cancellation_probe(|| true).unwrap();
            }
            let source: Box<dyn BatchOperator> = if mode == 2 {
                let bad = RecordBatch::try_from_iter(vec![(
                    "v",
                    Arc::new(arrow::array::StringArray::from(vec!["bad"; 20000])) as ArrayRef,
                )])
                .unwrap();
                Box::new(Input {
                    schema: Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)])),
                    batches: VecDeque::from([bad]),
                })
            } else {
                Box::new(Input::one(batch))
            };
            let mut operator = ParallelPartials::new(
                source,
                vec![],
                vec![AggExpr::new(AggFunc::Sum, "v")],
                pool.clone(),
                4,
            )
            .unwrap();
            let start = std::time::Instant::now();
            assert!(operator.next_batch().is_err());
            drop(operator);
            assert!(start.elapsed() < Duration::from_secs(3));
            assert_eq!(pool.snapshot().current_bytes, 0);
        }
    }

    #[test]
    fn query_parallelism_lowers_to_the_ceiling_and_never_raises() {
        let pool = QueryMemoryPool::new("q-ceiling", 1 << 20).unwrap();
        let configured = configured_parallelism().unwrap();
        assert_eq!(query_parallelism(None).unwrap(), configured);
        assert_eq!(query_parallelism(Some(&pool)).unwrap(), configured);
        set_query_parallelism(&pool, 1).unwrap();
        assert_eq!(query_parallelism(Some(&pool)).unwrap(), 1);
        let raised = QueryMemoryPool::new("q-raised", 1 << 20).unwrap();
        set_query_parallelism(&raised, configured + 8).unwrap();
        assert_eq!(query_parallelism(Some(&raised)).unwrap(), configured);
        assert!(set_query_parallelism(&raised, 0).is_err());
        assert!(set_query_parallelism(&raised, 1).is_err());
        set_query_parallelism(&raised, configured + 8).unwrap();
    }

    /// Keeps the rows of a binary key column whose key hashes to this
    /// thread and counts them, one batch out per batch in.
    struct KeepMine {
        source: Box<dyn BatchOperator>,
        selector: ThreadSelector,
        index: usize,
        schema: SchemaRef,
    }
    impl BatchOperator for KeepMine {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            let Some(batch) = self.source.next_batch()? else {
                return Ok(None);
            };
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .unwrap();
            let kept = (0..batch.num_rows())
                .filter(|row| self.selector.thread_of(keys.value(*row)) == self.index)
                .map(|row| keys.value(row).to_vec())
                .collect::<Vec<_>>();
            Ok(Some(
                RecordBatch::try_new(
                    self.schema.clone(),
                    vec![
                        Arc::new(arrow::array::BinaryArray::from_iter_values(kept.iter())),
                        Arc::new(Int64Array::from(vec![self.index as i64; kept.len()])),
                    ],
                )
                .unwrap(),
            ))
        }
    }

    #[test]
    fn broadcast_hands_every_batch_from_every_source_thread_to_every_thread_once() {
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("key", DataType::Binary, false),
        ]));
        let output_schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("key", DataType::Binary, false),
            arrow::datatypes::Field::new("thread", DataType::Int64, false),
        ]));
        // Three sources of four batches, 1000 keys each, all distinct.
        let sources = (0..3)
            .map(|source| {
                let schema = schema.clone();
                Box::new(move || {
                    let batches = (0..4)
                        .map(|batch| {
                            let keys = (0..1000)
                                .map(|i| format!("k{source}-{batch}-{i}").into_bytes())
                                .collect::<Vec<_>>();
                            RecordBatch::try_new(
                                schema.clone(),
                                vec![Arc::new(arrow::array::BinaryArray::from_iter_values(
                                    keys.iter(),
                                ))],
                            )
                            .unwrap()
                        })
                        .collect::<VecDeque<_>>();
                    Ok(Box::new(Unreserved(Box::new(Input {
                        schema: schema.clone(),
                        batches,
                    }))) as Box<dyn ThreadSource>)
                }) as SourceOpener
            })
            .collect::<Vec<_>>();
        let pool = QueryMemoryPool::new("broadcast", 64 << 20).unwrap();
        let thread_schema = output_schema.clone();
        let operator: ThreadOperator = Arc::new(move |source, _, context| {
            Ok(Box::new(KeepMine {
                source,
                selector: ThreadSelector::new(context.workers),
                index: context.index,
                schema: thread_schema.clone(),
            }) as Box<dyn BatchOperator>)
        });
        let mut parallel = ParallelPartials::broadcast(
            Sources::Threads {
                schema: schema.clone(),
                openers: sources,
            },
            output_schema,
            operator,
            pool.clone(),
            4,
        )
        .unwrap();
        let mut seen = std::collections::HashMap::<Vec<u8>, i64>::new();
        let mut batches = 0;
        while let Some(batch) = parallel.next_batch().unwrap() {
            batches += 1;
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::BinaryArray>()
                .unwrap();
            let threads = batch
                .column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            for row in 0..batch.num_rows() {
                assert!(
                    seen.insert(keys.value(row).to_vec(), threads.value(row))
                        .is_none(),
                    "a key reaches one thread"
                );
            }
        }
        // Every thread saw every one of the twelve batches.
        assert_eq!(batches, 12 * 4);
        assert_eq!(seen.len(), 12_000);
        let mut per_thread = [0; 4];
        for thread in seen.values() {
            per_thread[*thread as usize] += 1;
        }
        assert!(
            per_thread.iter().all(|count| *count > 2_000),
            "{per_thread:?}"
        );
        drop(parallel);
        assert_eq!(pool.snapshot().current_bytes, 0);

        // A source thread's failure is the operator's error.
        let failing = Sources::Threads {
            schema: schema.clone(),
            openers: vec![Box::new(|| Err(error("the spool is gone"))) as SourceOpener],
        };
        let operator: ThreadOperator = Arc::new(move |source, _, _| Ok(source));
        let mut parallel =
            ParallelPartials::broadcast(failing, schema, operator, pool.clone(), 2).unwrap();
        let failure = parallel.next_batch().unwrap_err();
        assert!(
            failure.to_string().contains("the spool is gone"),
            "{failure}"
        );
        drop(parallel);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    /// A request raised on a thread of its own, `since` read as a source
    /// reads it: before the attempt the refusal came from.
    fn request_on_a_thread(
        pressure: &Arc<Pressure>,
        account: &OperatorMemoryAccount,
        stopped: &Arc<AtomicBool>,
    ) -> JoinHandle<Result<Relief>> {
        let since = pressure.given_up();
        let pressure = Arc::clone(pressure);
        let account = account.clone();
        let stopped = Arc::clone(stopped);
        thread::spawn(move || pressure.request(since, &account, &stopped))
    }

    #[test]
    fn pressure_requests_resolve_on_the_first_spill_or_once_every_responder_declines() {
        let pool = QueryMemoryPool::new("pressure", 1 << 20).unwrap();
        let account = pool.operator("source").unwrap();
        let running = Arc::new(AtomicBool::new(false));

        // No thread answers: the refusal is final at once.
        let pressure = Pressure::new(2);
        pressure.settle(0);
        pressure.settle(1);
        assert_eq!(
            pressure.request(0, &account, &running).unwrap(),
            Relief::Nothing
        );

        // Two responders and one absent thread. The request waits for
        // the responders; the first with something to give claims it, the
        // other's answer changes nothing; the claimer's release resolves
        // it.
        let pressure = Pressure::new(3);
        let first = pressure.responder(0);
        let second = pressure.responder(1);
        pressure.settle(2);
        assert!(!first.pending());
        let waiter = request_on_a_thread(&pressure, &account, &running);
        while !first.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(second.pending());
        assert!(first.claim());
        assert!(!first.pending(), "answered");
        assert!(!second.claim(), "claimed already");
        assert!(!second.pending());
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished(), "the claimer has not released yet");
        first.released();
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Released);

        // A new request: both decline, and the refusal is final.
        let waiter = request_on_a_thread(&pressure, &account, &running);
        while !first.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        first.decline();
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished(), "one responder has not answered");
        second.decline();
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Nothing);

        // A thread that gives memory up for a refusal of its own while a
        // request is open answers it with that: no claim, no decline.
        let waiter = request_on_a_thread(&pressure, &account, &running);
        while !second.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        second.released();
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Released);
        assert!(!first.pending(), "resolved without the first's answer");

        // A refusal that predates a release is not the last word: every
        // thread declines, but memory was given up since the attempt.
        let since = pressure.given_up();
        first.released();
        let waiter = {
            let pressure = Arc::clone(&pressure);
            let account = account.clone();
            let running = Arc::clone(&running);
            thread::spawn(move || pressure.request(since, &account, &running))
        };
        while !first.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        first.decline();
        second.decline();
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Released);
        assert_eq!(
            pressure.stats(),
            PressureStats {
                requests: 4,
                released: 2,
                nothing: 2
            }
        );

        // A request raised before a thread's operator exists waits for
        // that thread to take a ticket or not.
        let pressure = Pressure::new(2);
        let ticket = pressure.responder(0);
        let waiter = request_on_a_thread(&pressure, &account, &running);
        while !ticket.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        ticket.decline();
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished(), "thread 1 is not settled");
        pressure.settle(1);
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Nothing);

        // A claimer that leaves (its ticket dropped) releases what it
        // held: the source asks again.
        let pressure = Pressure::new(1);
        let ticket = pressure.responder(0);
        let waiter = request_on_a_thread(&pressure, &account, &running);
        while !ticket.pending() {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(ticket.claim());
        drop(ticket);
        assert_eq!(waiter.join().unwrap().unwrap(), Relief::Released);
        assert_eq!(
            pressure.stats(),
            PressureStats {
                requests: 1,
                released: 1,
                nothing: 0
            }
        );

        // The stop and the query's cancellation end the wait.
        let pressure = Pressure::new(1);
        let _ticket = pressure.responder(0);
        let stopped = Arc::new(AtomicBool::new(false));
        let waiter = request_on_a_thread(&pressure, &account, &stopped);
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished());
        stopped.store(true, Ordering::Release);
        assert!(is_consequence(&waiter.join().unwrap().unwrap_err()));
        let cancelled = QueryMemoryPool::new("pressure-cancelled", 1 << 20).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let probe = Arc::clone(&flag);
        cancelled
            .set_cancellation_probe(move || probe.load(Ordering::Acquire))
            .unwrap();
        let account = cancelled.operator("source").unwrap();
        let waiter = request_on_a_thread(&pressure, &account, &running);
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished());
        flag.store(true, Ordering::Release);
        assert!(waiter.join().unwrap().is_err());
    }

    /// A source thread's refusal over an operator that does not answer
    /// pressure is final at once, and the source's batch is the one
    /// reservation for it while it is in flight.
    #[test]
    fn a_refused_source_over_a_thread_that_does_not_answer_fails_closed_at_once() {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        struct Refusing {
            schema: SchemaRef,
            account: OperatorMemoryAccount,
            bytes: u64,
            batch: Option<RecordBatch>,
        }
        impl ThreadSource for Refusing {
            fn schema(&self) -> &SchemaRef {
                &self.schema
            }
            fn next_batch(&mut self) -> Result<Option<ReservedBatch>> {
                if self.batch.is_none() {
                    return Ok(None);
                }
                let memory = self.account.reserve(self.bytes)?;
                Ok(Some(ReservedBatch {
                    batch: self.batch.take().expect("checked above"),
                    memory: Some(memory),
                }))
            }
        }
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from((0..1000).collect::<Vec<_>>()))],
        )
        .unwrap();
        let pool = QueryMemoryPool::new("refusing", 1 << 20).unwrap();
        let operator: ThreadOperator = Arc::new(move |source, _, _| Ok(source));
        for (bytes, admitted) in [(512u64 << 10, true), (2u64 << 20, false)] {
            let opener = {
                let schema = schema.clone();
                let batch = batch.clone();
                let account = pool.operator("source").unwrap();
                Box::new(move || {
                    Ok(Box::new(Refusing {
                        schema,
                        account,
                        bytes,
                        batch: Some(batch),
                    }) as Box<dyn ThreadSource>)
                }) as SourceOpener
            };
            let started = std::time::Instant::now();
            let mut parallel = ParallelPartials::broadcast(
                Sources::Threads {
                    schema: schema.clone(),
                    openers: vec![opener],
                },
                schema.clone(),
                operator.clone(),
                pool.clone(),
                2,
            )
            .unwrap();
            if admitted {
                let out = parallel.next_batch().unwrap().unwrap();
                assert_eq!(out.num_rows(), 1000);
                // The source's reservation, transferred: nothing else was
                // charged for the batch in flight, and the output's own.
                let held = pool.snapshot().current_bytes;
                assert!(
                    held <= bytes + 2 * (out.get_array_memory_size() as u64 + 8192),
                    "{held}"
                );
                assert_eq!(parallel.next_batch().unwrap().unwrap().num_rows(), 1000);
                assert!(parallel.next_batch().unwrap().is_none());
            } else {
                let error = parallel.next_batch().unwrap_err();
                assert!(matches!(error, KaveonError::MemoryLimit(_)), "{error}");
                assert!(
                    started.elapsed() < Duration::from_secs(2),
                    "no wait on a thread that cannot answer"
                );
            }
            drop(parallel);
            assert_eq!(pool.snapshot().current_bytes, 0);
        }
    }

    #[test]
    fn rendezvous_releases_when_every_party_arrives_or_is_dropped_and_leaves_on_cancellation() {
        let pool = QueryMemoryPool::new("rendezvous", 1 << 20).unwrap();
        let account = pool.operator("party").unwrap();
        // Three parties: two wait, the third's ticket is dropped unused.
        let rendezvous = Rendezvous::new(3);
        let tickets = (0..3).map(|_| rendezvous.ticket()).collect::<Vec<_>>();
        let mut tickets = tickets.into_iter();
        let first = tickets.next().unwrap();
        let second = tickets.next().unwrap();
        let dropped = tickets.next().unwrap();
        let waiter = {
            let account = account.clone();
            thread::spawn(move || first.wait(&account))
        };
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished(), "one party waits for the others");
        drop(dropped);
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished(), "a dropped party is not the last");
        second.wait(&account).unwrap();
        waiter.join().unwrap().unwrap();

        // A cancelled query leaves the rendezvous with the cancellation.
        let cancelled = QueryMemoryPool::new("rendezvous-cancelled", 1 << 20).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let probe = Arc::clone(&flag);
        cancelled
            .set_cancellation_probe(move || probe.load(Ordering::Acquire))
            .unwrap();
        let account = cancelled.operator("party").unwrap();
        let alone = Rendezvous::new(2);
        let ticket = alone.ticket();
        let _other = alone.ticket();
        let waiter = thread::spawn(move || ticket.wait(&account));
        thread::sleep(Duration::from_millis(30));
        assert!(!waiter.is_finished());
        flag.store(true, Ordering::Release);
        assert!(waiter.join().unwrap().is_err());
    }
}
