//! Parallel partial aggregation inside one task. Rows are hash-partitioned
//! by group key across aggregator threads, so every group lives in exactly
//! one thread's map and memory is the same as one aggregator's; ungrouped
//! aggregates round-robin. Operators are constructed inside their owning
//! threads.
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
        Arc,
        atomic::{AtomicBool, Ordering},
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
struct QueuedBatch {
    batch: RecordBatch,
    _memory: Arc<MemoryReservation>,
}
struct ChannelInput {
    schema: SchemaRef,
    receiver: Receiver<QueuedBatch>,
    stopped: Arc<AtomicBool>,
    current: Option<Arc<MemoryReservation>>,
}
impl BatchOperator for ChannelInput {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.current = None;
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return Err(error("parallel aggregate stopped"));
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
    /// Threads running side by side on the query budget.
    pub workers: usize,
    /// The query's spill budget and partition count when spilling is on.
    pub spill: Option<(SpillManager, usize)>,
}

/// One operator run on several threads within a task: rows go to the
/// thread their key hashes to, so the threads hold disjoint keys and their
/// outputs union without a merge. The source stays on its calling thread;
/// only Arrow batches cross thread boundaries. Grouped partial aggregates
/// and DISTINCT run this way.
pub struct ParallelPartials {
    source: Option<Box<dyn BatchOperator>>,
    schema: SchemaRef,
    keys: Vec<String>,
    /// Keys that are all dictionary-encoded hold a handful of values:
    /// cheaper to fold on every thread than to hash-partition every row.
    /// Only an operator whose outputs merge (a partial aggregate) may take
    /// that; DISTINCT must partition.
    fold_low_cardinality: bool,
    operator: ThreadOperator,
    pool: QueryMemoryPool,
    workers: usize,
    /// Each thread's operator spills through this when set: the query's
    /// shared spill budget and its partition count, decided once here so
    /// every thread takes the same path.
    spill: Option<(SpillManager, usize)>,
    stopped: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    /// One input channel per thread; empty once the source is drained.
    senders: Vec<SyncSender<QueuedBatch>>,
    partitioner: Option<HashPartitioner>,
    input_account: Option<OperatorMemoryAccount>,
    round_robin: usize,
    /// Parts of the current source batch not yet handed to their thread.
    pending: VecDeque<(usize, QueuedBatch)>,
    /// Outputs taken while a thread's input was full: an operator that
    /// streams (DISTINCT) fills the output queue before its input is
    /// drained, so the pump must take from one to push to the other.
    ready: VecDeque<QueuedBatch>,
    output: Option<Receiver<Result<QueuedBatch>>>,
    current: Option<Arc<MemoryReservation>>,
    failed: bool,
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
        Self::over(source, schema, groups, true, operator, pool, workers)
    }

    /// DISTINCT over `columns`: every thread deduplicates the rows whose
    /// values hash to it, so the union of the threads' outputs is distinct.
    pub fn distinct(
        source: Box<dyn BatchOperator>,
        columns: Vec<String>,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
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
        let operator: ThreadOperator = Arc::new(move |source, pool, _| {
            Ok(Box::new(
                DistinctOperator::new(source).with_memory(pool.operator("parallel-distinct")?),
            ) as Box<dyn BatchOperator>)
        });
        Self::over(source, schema, columns, false, operator, pool, workers)
    }

    fn over(
        source: Box<dyn BatchOperator>,
        schema: SchemaRef,
        keys: Vec<String>,
        fold_low_cardinality: bool,
        operator: ThreadOperator,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        if !(1..=MAX_WORKERS).contains(&workers) {
            return Err(error("parallel worker count must be between 1 and 16"));
        }
        let spill = spill_from_environment(&pool)?;
        Ok(Self {
            source: Some(source),
            schema,
            keys,
            fold_low_cardinality,
            operator,
            pool,
            workers,
            spill,
            stopped: Arc::new(AtomicBool::new(false)),
            handles: vec![],
            senders: Vec::new(),
            partitioner: None,
            input_account: None,
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
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| error("parallel source already consumed"))?;
        let (output_tx, output_rx) = mpsc::sync_channel(self.workers * 2);
        self.output = Some(output_rx);
        let mut senders = Vec::with_capacity(self.workers);
        for index in 0..self.workers {
            let (sender, receiver) = mpsc::sync_channel(2);
            senders.push(sender);
            let schema = source.schema().clone();
            let operator = self.operator.clone();
            let pool = self.pool.clone();
            let context = ThreadContext {
                workers: self.workers,
                spill: self.spill.clone(),
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
                                current: None,
                            });
                            run_worker(source, &operator, &pool, &context, &stopped, &output)
                        });
                        if let Err(err) = result {
                            let _ = output.send(Err(err));
                            stopped.store(true, Ordering::Release);
                        }
                    })
                    .map_err(|e| error(&format!("cannot spawn aggregate worker: {e}")))?,
            );
        }
        drop(output_tx);
        self.senders = senders;
        self.input_account = Some(self.pool.operator("parallel-input-queue")?);
        // Keyed: rows go to the thread their key hashes to, so the threads
        // hold disjoint keys. Unkeyed, or keyed only by dictionary columns
        // where folding is allowed (a handful of groups, cheaper to fold N
        // times than to hash-partition every row): slices round-robin.
        let low_cardinality_keys = self.fold_low_cardinality
            && self.keys.iter().all(|name| {
                source
                    .schema()
                    .field_with_name(name)
                    .is_ok_and(|field| matches!(field.data_type(), DataType::Dictionary(_, _)))
            });
        self.partitioner = if self.keys.is_empty() || self.workers == 1 || low_cardinality_keys {
            None
        } else {
            Some(HashPartitioner::try_new_salted(
                source.schema(),
                &self.keys,
                self.workers,
                crate::exchange::THREAD_PARTITION_SALT,
            )?)
        };
        Ok(())
    }

    /// Move one source batch to the threads. Returns false once the source
    /// is drained and the threads' inputs are closed. A full input queue is
    /// never waited on blindly: outputs are taken meanwhile, so a thread
    /// blocked on a full output queue is unblocked by the same loop.
    fn pump(&mut self) -> Result<bool> {
        let account = self
            .input_account
            .clone()
            .ok_or_else(|| error("parallel operator not started"))?;
        if self.pending.is_empty() {
            let Some(source) = self.source.as_mut() else {
                return Ok(false);
            };
            let Some(batch) = source.next_batch()? else {
                self.source = None;
                self.senders.clear();
                return Ok(false);
            };
            if batch.schema() != *source.schema() {
                return Err(error(
                    "parallel source batch does not match declared schema",
                ));
            }
            account.check_cancelled()?;
            match &self.partitioner {
                Some(partitioner) => {
                    for (worker, part) in partitioner.partition(&batch)?.into_iter().enumerate() {
                        if part.num_rows() == 0 {
                            continue;
                        }
                        let memory =
                            Arc::new(account.reserve(part.get_array_memory_size() as u64)?);
                        self.pending.push_back((
                            worker,
                            QueuedBatch {
                                batch: part,
                                _memory: memory,
                            },
                        ));
                    }
                }
                None => {
                    let memory = Arc::new(account.reserve(batch.get_array_memory_size() as u64)?);
                    for offset in (0..batch.num_rows()).step_by(8192) {
                        let slice = batch.slice(offset, 8192.min(batch.num_rows() - offset));
                        self.pending.push_back((
                            self.round_robin % self.workers,
                            QueuedBatch {
                                batch: slice,
                                _memory: memory.clone(),
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
                return Err(error("parallel operator stopped"));
            }
            match self.senders[worker].try_send(queued) {
                Ok(()) => {}
                Err(TrySendError::Disconnected(_)) => {
                    return Err(error("parallel operator channel disconnected"));
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
        for handle in self.handles.drain(..) {
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
            if self.source.is_some() && self.output.is_none() {
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
        let result = result.map_err(|error| {
            self.output
                .as_ref()
                .and_then(|output| output.try_iter().find_map(|message| message.err()))
                .unwrap_or(error)
        });
        if result.is_err() {
            self.failed = true;
            self.stop();
        }
        result
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
            return Err(error("parallel aggregate stopped"));
        }
        match sender.try_send(value) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err(error("parallel aggregate channel disconnected"));
            }
            Err(TrySendError::Full(returned)) => {
                value = returned;
                thread::sleep(Duration::from_millis(1));
            }
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
    while let Some(batch) = operator.next_batch()? {
        // The batch is held by the queue; whatever it came from is not.
        let memory = Arc::new(account.reserve(batch.get_array_memory_size() as u64 + 8192)?);
        send_bounded(
            output,
            Ok(QueuedBatch {
                batch,
                _memory: memory,
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
            let parallel = collect(Box::new(
                ParallelPartials::distinct(
                    Box::new(Input { schema, batches }),
                    columns,
                    pool.clone(),
                    4,
                )
                .unwrap(),
            ));
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
}
