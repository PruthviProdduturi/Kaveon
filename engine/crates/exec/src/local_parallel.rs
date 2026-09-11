//! Opt-in local aggregate workers. Operators are constructed inside their owning threads.
use crate::{
    aggregate::{
        AggExpr, HashAggregate, aggregate_metrics, aggregate_output_types,
        grouped_aggregate_states_to_schema_batch,
    },
    exchange::HashPartitioner,
    partitioned::{PartitionedHashAggregate, spill_from_environment},
};
use arrow::{datatypes::SchemaRef, record_batch::RecordBatch};
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, QueryMemoryPool, Result};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_WORKERS: usize = 16;
const AFFINITY_SAMPLE_BATCHES: usize = 8;
const AFFINITY_SAMPLE_ROWS: usize = 64 * 1024;
const AFFINITY_DISTINCT_THRESHOLD: usize = 4 * 1024;
const MAX_AFFINITY_SKEW: usize = 2;
pub fn configured_parallelism() -> Result<usize> {
    let value = match std::env::var("KAVEON_LOCAL_PARALLELISM") {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|_| error("KAVEON_LOCAL_PARALLELISM must be a positive integer"))?,
        Err(std::env::VarError::NotPresent) => 1,
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

struct DispatchSample {
    batches: Vec<QueuedBatch>,
    rows: usize,
    distinct_hashes: usize,
    partition_rows: Vec<usize>,
}

impl DispatchSample {
    fn use_affinity(&self, workers: usize, grouped: bool) -> bool {
        if !grouped
            || workers <= 1
            || self.rows == 0
            || self.distinct_hashes < AFFINITY_DISTINCT_THRESHOLD
        {
            return false;
        }
        let largest = self.partition_rows.iter().copied().max().unwrap_or(0);
        largest.saturating_mul(workers) <= self.rows.saturating_mul(MAX_AFFINITY_SKEW)
    }
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
/// The source stays on its calling thread; only Arrow batches cross thread boundaries.
pub struct ParallelPartials {
    source: Option<Box<dyn BatchOperator>>,
    schema: SchemaRef,
    groups: Vec<String>,
    aggregates: Vec<AggExpr>,
    pool: QueryMemoryPool,
    workers: usize,
    stopped: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    output: Option<Receiver<Result<QueuedBatch>>>,
    current: Option<Arc<MemoryReservation>>,
    failed: bool,
}
impl ParallelPartials {
    pub fn new(
        source: Box<dyn BatchOperator>,
        groups: Vec<String>,
        aggregates: Vec<AggExpr>,
        pool: QueryMemoryPool,
        workers: usize,
    ) -> Result<Self> {
        if !(1..=MAX_WORKERS).contains(&workers) {
            return Err(error("parallel worker count must be between 1 and 16"));
        }
        let types = aggregate_output_types(&aggregates, source.schema())?;
        let keys = groups
            .iter()
            .map(|name| {
                source
                    .schema()
                    .field_with_name(name)
                    .map(|f| f.data_type().clone())
                    .map_err(KaveonError::from)
            })
            .collect::<Result<Vec<_>>>()?;
        let schema = grouped_aggregate_states_to_schema_batch(&[], &keys, &types)?.schema();
        // Validate source bindings before any threads are started.
        HashAggregate::new(
            Box::new(EmptyInput(source.schema().clone())),
            groups.clone(),
            aggregates.clone(),
        )?;
        Ok(Self {
            source: Some(source),
            schema,
            groups,
            aggregates,
            pool,
            workers,
            stopped: Arc::new(AtomicBool::new(false)),
            handles: vec![],
            output: None,
            current: None,
            failed: false,
        })
    }
    fn start(&mut self) -> Result<()> {
        let mut source = self
            .source
            .take()
            .ok_or_else(|| error("parallel source already consumed"))?;
        let account = self.pool.operator("parallel-input-queue")?;
        let partitioner = (!self.groups.is_empty() && self.workers > 1)
            .then(|| HashPartitioner::try_new(source.schema(), &self.groups, self.workers))
            .transpose()?;
        let sample = collect_dispatch_sample(
            source.as_mut(),
            partitioner.as_ref(),
            self.workers,
            &account,
        )?;
        let affinity = sample.use_affinity(self.workers, partitioner.is_some());
        aggregate_metrics(&self.pool)?.record_local_dispatch(
            affinity,
            sample.rows as u64,
            sample.distinct_hashes as u64,
        );
        let (output_tx, output_rx) = mpsc::sync_channel(self.workers * 2);
        self.output = Some(output_rx);
        let mut senders = Vec::with_capacity(self.workers);
        for index in 0..self.workers {
            let (sender, receiver) = mpsc::sync_channel(2);
            senders.push(sender);
            let schema = source.schema().clone();
            let groups = self.groups.clone();
            let aggregates = self.aggregates.clone();
            let pool = self.pool.clone();
            let stopped = self.stopped.clone();
            let output = output_tx.clone();
            self.handles.push(
                thread::Builder::new()
                    .name(format!("kaveon-aggregate-{index}"))
                    .spawn(move || {
                        let result = catch_worker_failure(|| {
                            let source = Box::new(ChannelInput {
                                schema,
                                receiver,
                                stopped: stopped.clone(),
                                current: None,
                            });
                            run_worker(source, groups, aggregates, &pool, &stopped, &output)
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
        let mut index = 0;
        for queued in sample.batches {
            dispatch_batch(
                queued,
                affinity.then_some(partitioner.as_ref()).flatten(),
                &senders,
                &mut index,
                &account,
                &self.stopped,
                &self.pool,
            )?;
        }
        while let Some(batch) = source.next_batch()? {
            if batch.schema() != *source.schema() {
                return Err(error(
                    "parallel source batch does not match declared schema",
                ));
            }
            account.check_cancelled()?;
            let memory = Arc::new(account.reserve(batch.get_array_memory_size() as u64)?);
            dispatch_batch(
                QueuedBatch {
                    batch,
                    _memory: memory,
                },
                affinity.then_some(partitioner.as_ref()).flatten(),
                &senders,
                &mut index,
                &account,
                &self.stopped,
                &self.pool,
            )?;
        }
        drop(senders);
        Ok(())
    }
    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.output = None;
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        self.current = None;
    }
}

fn collect_dispatch_sample(
    source: &mut dyn BatchOperator,
    partitioner: Option<&HashPartitioner>,
    workers: usize,
    memory: &kaveon_core::OperatorMemoryAccount,
) -> Result<DispatchSample> {
    let mut batches = Vec::new();
    let mut rows = 0usize;
    let mut distinct = HashSet::with_capacity(AFFINITY_DISTINCT_THRESHOLD);
    let mut partition_rows = vec![0usize; workers];
    let _sample_state_memory = partitioner
        .map(|_| {
            memory.reserve(
                (AFFINITY_DISTINCT_THRESHOLD as u64)
                    .saturating_mul(32)
                    .saturating_add((workers as u64).saturating_mul(64)),
            )
        })
        .transpose()?;
    while batches.len() < AFFINITY_SAMPLE_BATCHES && rows < AFFINITY_SAMPLE_ROWS {
        let Some(batch) = source.next_batch()? else {
            break;
        };
        if batch.schema() != *source.schema() {
            return Err(error(
                "parallel source batch does not match declared schema",
            ));
        }
        memory.check_cancelled()?;
        let guard = Arc::new(memory.reserve(batch.get_array_memory_size() as u64)?);
        if let Some(partitioner) = partitioner {
            let _hash_memory = memory.reserve(
                (batch.num_rows() as u64)
                    .saturating_mul(64)
                    .saturating_add(4096),
            )?;
            for hash in partitioner.hashes(&batch)? {
                let partition = (hash % workers as u64) as usize;
                partition_rows[partition] = partition_rows[partition].saturating_add(1);
                if distinct.len() < AFFINITY_DISTINCT_THRESHOLD {
                    distinct.insert(hash);
                }
            }
        }
        rows = rows.saturating_add(batch.num_rows());
        batches.push(QueuedBatch {
            batch,
            _memory: guard,
        });
    }
    Ok(DispatchSample {
        batches,
        rows,
        distinct_hashes: distinct.len(),
        partition_rows,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch_batch(
    queued: QueuedBatch,
    partitioner: Option<&HashPartitioner>,
    senders: &[SyncSender<QueuedBatch>],
    round_robin_index: &mut usize,
    memory: &kaveon_core::OperatorMemoryAccount,
    stopped: &AtomicBool,
    pool: &QueryMemoryPool,
) -> Result<()> {
    if queued.batch.num_rows() == 0 {
        return Ok(());
    }
    if let Some(partitioner) = partitioner {
        let batch_bytes = queued.batch.get_array_memory_size() as u64;
        let estimate = batch_bytes
            .checked_mul(3)
            .and_then(|bytes| {
                bytes.checked_add((queued.batch.num_rows() as u64).saturating_mul(32))
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    (senders.len() as u64)
                        .saturating_mul(queued.batch.num_columns() as u64)
                        .saturating_mul(256),
                )
            })
            .ok_or_else(|| error("local aggregate partition memory estimate overflow"))?;
        let routing_memory = Arc::new(memory.reserve(estimate)?);
        let (partitions, metrics) = partitioner.partition_profiled(&queued.batch)?;
        aggregate_metrics(pool)?
            .record_local_affinity_routing(queued.batch.num_rows() as u64, metrics.copied_bytes);
        drop(queued);
        for (worker, batch) in partitions.into_iter().enumerate() {
            for offset in (0..batch.num_rows()).step_by(8192) {
                send_bounded(
                    &senders[worker],
                    QueuedBatch {
                        batch: batch.slice(offset, 8192.min(batch.num_rows() - offset)),
                        _memory: routing_memory.clone(),
                    },
                    stopped,
                    pool,
                )?;
            }
        }
    } else {
        for offset in (0..queued.batch.num_rows()).step_by(8192) {
            send_bounded(
                &senders[*round_robin_index % senders.len()],
                QueuedBatch {
                    batch: queued
                        .batch
                        .slice(offset, 8192.min(queued.batch.num_rows() - offset)),
                    _memory: queued._memory.clone(),
                },
                stopped,
                pool,
            )?;
            *round_robin_index = (*round_robin_index).saturating_add(1);
        }
    }
    Ok(())
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
            if self.source.is_some() {
                self.start()?;
            }
            let account = self.pool.operator("parallel-output-queue")?;
            loop {
                account.check_cancelled()?;
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
    groups: Vec<String>,
    aggregates: Vec<AggExpr>,
    pool: &QueryMemoryPool,
    stopped: &AtomicBool,
    output: &SyncSender<Result<QueuedBatch>>,
) -> Result<()> {
    let account = pool.operator("parallel-partial-aggregate")?;
    if let Some((spill, count)) = spill_from_environment(pool)? {
        let mut operator = PartitionedHashAggregate::new_partial(
            source,
            groups,
            aggregates,
            account.clone(),
            spill,
            count,
        )?
        .with_reserved_input();
        while let Some(batch) = operator.next_batch()? {
            let memory = Arc::new(account.reserve(batch.get_array_memory_size() as u64)?);
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
    } else {
        let types = aggregate_output_types(&aggregates, source.schema())?;
        let keys = groups
            .iter()
            .map(|name| {
                source
                    .schema()
                    .field_with_name(name)
                    .map(|f| f.data_type().clone())
                    .map_err(KaveonError::from)
            })
            .collect::<Result<Vec<_>>>()?;
        let operator = HashAggregate::new_with_memory(source, groups, aggregates, account.clone())?
            .with_reserved_input();
        let (states, guards) = operator.into_grouped_states_with_reservations()?;
        let bytes = guards
            .iter()
            .map(MemoryReservation::bytes)
            .sum::<u64>()
            .saturating_mul(4)
            .saturating_add((states.len() as u64).saturating_mul(4096))
            .saturating_add(8192);
        let memory = Arc::new(account.reserve(bytes)?);
        let batch = grouped_aggregate_states_to_schema_batch(&states, &keys, &types)?;
        drop(states);
        drop(guards);
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
        AggFunc, AggregateValue, FinalAggregateValue, finalize_grouped_aggregate_states,
        grouped_aggregate_states_from_batches, merge_grouped_aggregate_states,
    };
    use arrow::{
        array::{ArrayRef, Decimal128Array, Float64Array, Int32Array, Int64Array, UInt64Array},
        datatypes::{DataType, Field, Schema},
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
    fn affinity_gate_requires_high_cardinality_balanced_grouping() {
        let high = DispatchSample {
            batches: vec![],
            rows: 16_384,
            distinct_hashes: AFFINITY_DISTINCT_THRESHOLD,
            partition_rows: vec![4096; 4],
        };
        assert!(high.use_affinity(4, true));
        assert!(!high.use_affinity(4, false));
        assert!(!high.use_affinity(1, true));

        let low = DispatchSample {
            distinct_hashes: 17,
            ..high
        };
        assert!(!low.use_affinity(4, true));

        let skewed = DispatchSample {
            batches: vec![],
            rows: 16_384,
            distinct_hashes: AFFINITY_DISTINCT_THRESHOLD,
            partition_rows: vec![13_000, 1128, 1128, 1128],
        };
        assert!(!skewed.use_affinity(4, true));
    }

    #[test]
    fn high_cardinality_groups_are_key_affine_and_exact() {
        let rows = 20_000i64;
        let batch = RecordBatch::try_from_iter(vec![
            (
                "k",
                Arc::new(Int64Array::from_iter_values(0..rows)) as ArrayRef,
            ),
            (
                "v",
                Arc::new(Int64Array::from_iter_values(
                    (0..rows).map(|value| value.saturating_mul(-3)),
                )) as ArrayRef,
            ),
        ])
        .unwrap();
        let pool = QueryMemoryPool::new("key-affine", 256 * 1024 * 1024).unwrap();
        let mut operator = ParallelPartials::new(
            Box::new(Input::one(batch)),
            vec!["k".into()],
            vec![
                AggExpr::new(AggFunc::Count, "*"),
                AggExpr::new(AggFunc::Sum, "v"),
            ],
            pool.clone(),
            4,
        )
        .unwrap();
        let mut partials = Vec::new();
        while let Some(batch) = operator.next_batch().unwrap() {
            partials.extend(grouped_aggregate_states_from_batches(&[batch]).unwrap());
        }
        let actual =
            finalize_grouped_aggregate_states(&merge_grouped_aggregate_states(partials).unwrap())
                .unwrap();
        assert_eq!(actual.len(), rows as usize);
        assert!(actual.iter().all(|group| {
            let AggregateValue::Int64(key) = group.group_keys[0] else {
                return false;
            };
            group.values
                == vec![
                    FinalAggregateValue::Count(1),
                    FinalAggregateValue::Integer(Some(key.saturating_mul(-3) as i128)),
                ]
        }));
        let metrics = aggregate_metrics(&pool).unwrap().snapshot();
        assert_eq!(metrics.local_affinity_dispatches, 1);
        assert_eq!(metrics.local_round_robin_dispatches, 0);
        assert_eq!(metrics.local_affinity_sample_rows, rows as u64);
        assert_eq!(metrics.local_affinity_sample_distinct_hashes, 4096);
        assert_eq!(metrics.local_affinity_routed_rows, rows as u64);
        assert!(metrics.local_affinity_routed_bytes > 0);
        drop(operator);
        assert_eq!(pool.snapshot().current_bytes, 0);
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
