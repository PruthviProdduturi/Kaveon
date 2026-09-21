use std::collections::{BTreeMap, BTreeSet, VecDeque};

use kaveon_core::{
    ExchangeId, ExecutableFragment, KaveonError, Partitioning, Result, StageGraph, StageId,
    TaskAssignment, TaskId, TaskState,
};
use serde::Serialize;

use crate::cluster::NodeInfo;
use crate::runtime::{ExchangeCleanupIntent, StageRuntime, SupersededState};

const DEFAULT_MAX_TASK_ATTEMPTS: u32 = 3;
/// How many times one stage's finished output may be produced again for
/// one query because the worker holding its consumers' spools was lost
/// (`KAVEON_STAGE_RETRY_LIMIT`).
pub const DEFAULT_STAGE_RETRY_LIMIT: u32 = 2;

/// One attempt a worker loss created: the slot, the worker it was on, the
/// worker it runs on now, and the attempt number it runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StageRetry {
    pub stage: u32,
    pub partition: usize,
    pub from: String,
    pub to: String,
    pub attempt: u32,
}

/// A consumer's exchange spool moved off a lost worker: the producers'
/// output for this consumer partition is written to `to` from now on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpoolMove {
    pub stage: u32,
    pub partition: usize,
    pub from: String,
    pub to: String,
}

/// What the loss of one worker cost the query: every attempt it created
/// and every spool it moved. Empty when the worker was already gone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WorkerLossRecovery {
    pub worker: String,
    pub moved_spools: Vec<SpoolMove>,
    pub stage_retries: Vec<StageRetry>,
    /// The stages whose finished output was produced again.
    pub reexecuted_stages: Vec<u32>,
}

impl WorkerLossRecovery {
    pub fn is_empty(&self) -> bool {
        self.moved_spools.is_empty() && self.stage_retries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeLocation {
    pub exchange_id: ExchangeId,
    pub producer: TaskId,
    pub output_partition: usize,
    pub worker_uri: String,
}

#[derive(Clone, Debug)]
pub struct TaskDispatch {
    pub assignment: TaskAssignment,
    /// The worker must apply this partition to every storage scan in the fragment.
    /// Ignoring it would make each task scan the entire table and duplicate results.
    pub execution_partition: ExecutionPartition,
    pub fragment: ExecutableFragment,
    pub exchange_inputs: Vec<ExchangeLocation>,
    pub exchange_outputs: Vec<ExchangeLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionPartition {
    pub index: usize,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeCleanup {
    pub exchange_id: ExchangeId,
    pub locations: Vec<ExchangeLocation>,
}

/// Coordinator-side control plane for a planned stage DAG.
///
/// This type deliberately stops at the transport boundary. The caller submits each
/// [`TaskDispatch`] to its worker, reports completion or failure, and performs the
/// returned exchange cleanup. Keeping HTTP out of this state machine makes retries
/// and dependency transitions deterministic and directly testable.
pub struct CoordinatorOrchestrator {
    graph: StageGraph,
    fragments: BTreeMap<StageId, ExecutableFragment>,
    workers: Vec<NodeInfo>,
    assignments: BTreeMap<(StageId, usize), TaskAssignment>,
    exchange_workers: BTreeMap<(StageId, usize), String>,
    runtime: StageRuntime,
    exchange_store_uri: Option<String>,
    stage_retry_limit: u32,
    /// How many times each stage's finished output was produced again
    /// after a worker loss.
    stage_reexecutions: BTreeMap<StageId, u32>,
}

impl CoordinatorOrchestrator {
    pub fn new(
        graph: StageGraph,
        fragments: BTreeMap<StageId, ExecutableFragment>,
        workers: Vec<NodeInfo>,
    ) -> Result<Self> {
        Self::with_max_task_attempts(graph, fragments, workers, DEFAULT_MAX_TASK_ATTEMPTS)
    }

    pub fn with_max_task_attempts(
        graph: StageGraph,
        fragments: BTreeMap<StageId, ExecutableFragment>,
        mut workers: Vec<NodeInfo>,
        max_task_attempts: u32,
    ) -> Result<Self> {
        graph.validate()?;
        validate_fragments(&graph, &fragments)?;
        workers.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        validate_workers(&workers)?;

        let assignments = build_assignments(&graph, &workers);
        let runtime = StageRuntime::with_max_task_attempts(
            graph.clone(),
            assignments.values().cloned().collect(),
            max_task_attempts,
        )?;
        let exchange_workers = assignments
            .iter()
            .map(|(key, assignment)| (*key, assignment.worker_id.clone()))
            .collect();
        Ok(Self {
            exchange_store_uri: None,
            exchange_workers,
            graph,
            fragments,
            workers,
            assignments,
            runtime,
            stage_retry_limit: DEFAULT_STAGE_RETRY_LIMIT,
            stage_reexecutions: BTreeMap::new(),
        })
    }

    pub fn set_exchange_store_uri(&mut self, uri: String) {
        self.exchange_store_uri = Some(uri);
    }

    /// How many times one stage may be re-executed for this query after
    /// worker losses; zero forbids stage re-execution (a task retry on a
    /// surviving worker still happens).
    pub fn set_stage_retry_limit(&mut self, limit: u32) {
        self.stage_retry_limit = limit;
    }

    /// The workers the query may still dispatch to.
    pub fn live_workers(&self) -> &[NodeInfo] {
        &self.workers
    }

    /// Whether `task_id` is the attempt its slot stands on now; the
    /// outcome of an older attempt is stale and must be dropped.
    pub fn is_current_attempt(&self, task_id: &TaskId) -> bool {
        self.runtime.is_current_attempt(task_id)
    }

    /// Recover from the loss of `worker_id`: it is dispatched to no more;
    /// the tasks it was running or about to run move to surviving workers
    /// (a running one as the next attempt); every consumer spool placed on
    /// it moves to a surviving worker, and the producers of each such
    /// consumer partition that has not finished run again as the next
    /// attempt on the surviving workers, so the moved spool is filled
    /// again — a producer stage whose own inputs were released when it
    /// finished re-executes its producers too, down to the scans. A
    /// consumer that already holds its complete input keeps it and is
    /// left running. Each stage's finished output may be produced again
    /// at most `stage_retry_limit` times per query; beyond that the loss
    /// fails the query, naming the worker and the stage. When the
    /// coordinator relays exchanges (`set_exchange_store_uri`) no spool
    /// is on a worker and only the tasks move.
    pub fn lose_worker(&mut self, worker_id: &str) -> Result<WorkerLossRecovery> {
        let mut recovery = WorkerLossRecovery {
            worker: worker_id.to_owned(),
            ..WorkerLossRecovery::default()
        };
        let Some(index) = self
            .workers
            .iter()
            .position(|worker| worker.node_id == worker_id)
        else {
            return Ok(recovery);
        };
        self.workers.remove(index);
        if self.workers.is_empty() {
            self.runtime.cancel();
            return Err(execution_error(&format!(
                "worker '{worker_id}' is lost and no worker survives"
            )));
        }
        let reason = format!("worker '{worker_id}' was lost");

        // The spools placed on the lost worker, and where they live now.
        // A stage without inputs has no spool, whatever its placement.
        let mut moved = BTreeSet::new();
        if self.exchange_store_uri.is_none() {
            let survivors = self.workers.clone();
            let consuming = self
                .graph
                .exchanges
                .iter()
                .map(|exchange| exchange.target_stage)
                .collect::<BTreeSet<_>>();
            for ((stage_id, partition), placement) in &mut self.exchange_workers {
                if placement != worker_id || !consuming.contains(stage_id) {
                    continue;
                }
                let to = survivors[*partition % survivors.len()].node_id.clone();
                recovery.moved_spools.push(SpoolMove {
                    stage: stage_id.0,
                    partition: *partition,
                    from: worker_id.to_owned(),
                    to: to.clone(),
                });
                *placement = to;
                moved.insert((*stage_id, *partition));
            }
        }

        // The tasks the lost worker was running or about to run.
        let on_lost_worker = self
            .runtime
            .slots()
            .filter(|(assignment, state)| {
                assignment.worker_id == worker_id
                    && matches!(state, TaskState::Pending | TaskState::Running)
            })
            .map(|(assignment, _)| (assignment.task_id.stage_id, assignment.task_id.partition))
            .collect::<Vec<_>>();
        for (stage_id, partition) in on_lost_worker {
            self.supersede(stage_id, partition, &reason, &mut recovery)?;
        }

        // The producers of every moved spool whose consumer still needs it.
        let mut queue = VecDeque::new();
        for (stage_id, partition) in &moved {
            let needed = self.runtime.slots().any(|(assignment, state)| {
                assignment.task_id.stage_id == *stage_id
                    && assignment.task_id.partition == *partition
                    && state != TaskState::Finished
            });
            if needed {
                queue.extend(self.producer_stages(*stage_id));
            }
        }
        let mut reopened = BTreeSet::new();
        while let Some(stage_id) = queue.pop_front() {
            if !reopened.insert(stage_id) {
                continue;
            }
            let limit = self.stage_retry_limit;
            let count = self.stage_reexecutions.entry(stage_id).or_default();
            if *count >= limit {
                let attempt = ordinal(*count + 1);
                self.runtime.cancel();
                return Err(execution_error(&format!(
                    "worker '{worker_id}' was lost and stage {} would be re-executed a {attempt} time for this query, beyond KAVEON_STAGE_RETRY_LIMIT={limit}",
                    stage_id.0
                )));
            }
            *count += 1;
            recovery.reexecuted_stages.push(stage_id.0);
            let was_finished =
                self.runtime.stage_state(stage_id) == Some(crate::runtime::StageState::Finished);
            let slots = self
                .runtime
                .slots()
                .filter(|(assignment, state)| {
                    assignment.task_id.stage_id == stage_id
                        && matches!(state, TaskState::Running | TaskState::Finished)
                })
                .map(|(assignment, _)| assignment.task_id.partition)
                .collect::<Vec<_>>();
            for partition in slots {
                self.supersede(stage_id, partition, &reason, &mut recovery)?;
            }
            // Its inputs are gone when it had finished (released with its
            // finish) or when one of its own spools was on the lost worker.
            let inputs_gone = was_finished
                || moved
                    .iter()
                    .any(|(moved_stage, _)| *moved_stage == stage_id);
            if inputs_gone {
                queue.extend(self.producer_stages(stage_id));
            }
        }
        Ok(recovery)
    }

    fn producer_stages(&self, stage_id: StageId) -> Vec<StageId> {
        self.graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.target_stage == stage_id)
            .map(|exchange| exchange.source_stage)
            .collect()
    }

    fn supersede(
        &mut self,
        stage_id: StageId,
        partition: usize,
        reason: &str,
        recovery: &mut WorkerLossRecovery,
    ) -> Result<()> {
        let current = self
            .assignments
            .get(&(stage_id, partition))
            .ok_or_else(|| execution_error("worker loss references an unknown task slot"))?
            .clone();
        let to = self.next_worker(partition, &current.worker_id);
        let (was, next) = self
            .runtime
            .supersede_task(stage_id, partition, to.clone(), reason)?;
        self.assignments.insert((stage_id, partition), next.clone());
        if was != SupersededState::Pending {
            recovery.stage_retries.push(StageRetry {
                stage: stage_id.0,
                partition,
                from: current.worker_id,
                to,
                attempt: next.task_id.attempt,
            });
        }
        Ok(())
    }

    /// The surviving worker after `current` in rotation; `partition`'s own
    /// rotation slot when `current` is gone.
    fn next_worker(&self, partition: usize, current: &str) -> String {
        match self
            .workers
            .iter()
            .position(|worker| worker.node_id == current)
        {
            Some(index) => self.workers[(index + 1) % self.workers.len()]
                .node_id
                .clone(),
            None => self.workers[partition % self.workers.len()].node_id.clone(),
        }
    }

    pub fn ready_dispatches(&self) -> Result<Vec<TaskDispatch>> {
        self.runtime
            .ready_tasks()
            .into_iter()
            .map(|assignment| self.dispatch(assignment))
            .collect()
    }

    pub fn start_task(&mut self, task_id: &TaskId) -> Result<()> {
        self.runtime.start_task(task_id)
    }

    pub fn finish_task(&mut self, task_id: &TaskId) -> Result<()> {
        self.runtime.finish_task(task_id)
    }

    pub fn fail_task(&mut self, task_id: &TaskId, failure: impl Into<String>) -> Result<bool> {
        let retry_worker = self.retry_worker(task_id);
        let retry = self.runtime.fail_task(task_id, failure, retry_worker)?;
        if let Some(assignment) = retry {
            self.assignments.insert(
                (assignment.task_id.stage_id, assignment.task_id.partition),
                assignment,
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn cancel(&mut self) {
        self.runtime.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.runtime.is_finished()
    }

    pub fn is_terminal(&self) -> bool {
        self.runtime.is_terminal()
    }

    pub fn drain_cleanup_intents(&mut self) -> Vec<ExchangeCleanupIntent> {
        self.runtime.drain_cleanup_intents()
    }

    pub fn drain_exchange_cleanup(&mut self) -> Result<Vec<ExchangeCleanup>> {
        self.runtime
            .drain_cleanup_intents()
            .into_iter()
            .map(|intent| {
                let exchange = self
                    .graph
                    .exchanges
                    .iter()
                    .find(|exchange| exchange.id == intent.exchange_id)
                    .ok_or_else(|| execution_error("cleanup references an unknown exchange"))?;
                let mut locations = Vec::new();
                for producer in self.stage_assignments(exchange.source_stage) {
                    for consumer in self.stage_assignments(exchange.target_stage) {
                        let output_partition = match exchange.partitioning {
                            Partitioning::Single => 0,
                            Partitioning::Broadcast => 0,
                            Partitioning::Hash { .. } | Partitioning::RoundRobin { .. } => {
                                consumer.task_id.partition
                            }
                        };
                        locations.push(ExchangeLocation {
                            exchange_id: exchange.id.clone(),
                            producer: producer.task_id.clone(),
                            output_partition,
                            worker_uri: self.exchange_worker_uri(consumer)?,
                        });
                    }
                }
                Ok(ExchangeCleanup {
                    exchange_id: intent.exchange_id,
                    locations,
                })
            })
            .collect()
    }

    fn dispatch(&self, assignment: TaskAssignment) -> Result<TaskDispatch> {
        let fragment = self
            .fragments
            .get(&assignment.task_id.stage_id)
            .ok_or_else(|| execution_error("task references a missing executable fragment"))?
            .clone();
        let exchange_inputs = self.input_locations(&assignment)?;
        let exchange_outputs = self.output_locations(&assignment)?;
        let execution_partition = ExecutionPartition {
            index: assignment.task_id.partition,
            count: self.stage_task_count(assignment.task_id.stage_id)?,
        };
        Ok(TaskDispatch {
            assignment,
            execution_partition,
            fragment,
            exchange_inputs,
            exchange_outputs,
        })
    }

    fn input_locations(&self, assignment: &TaskAssignment) -> Result<Vec<ExchangeLocation>> {
        let mut locations = Vec::new();
        for exchange in self
            .graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.target_stage == assignment.task_id.stage_id)
        {
            let output_partition = match exchange.partitioning {
                Partitioning::Single => 0,
                Partitioning::Broadcast => 0,
                Partitioning::Hash { .. } | Partitioning::RoundRobin { .. } => {
                    assignment.task_id.partition
                }
            };
            for producer in self.stage_assignments(exchange.source_stage) {
                locations.push(ExchangeLocation {
                    exchange_id: exchange.id.clone(),
                    producer: producer.task_id.clone(),
                    output_partition,
                    worker_uri: self.exchange_worker_uri(assignment)?,
                });
            }
        }
        Ok(locations)
    }

    fn output_locations(&self, assignment: &TaskAssignment) -> Result<Vec<ExchangeLocation>> {
        let mut locations = Vec::new();
        for exchange in self
            .graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.source_stage == assignment.task_id.stage_id)
        {
            for consumer in self.stage_assignments(exchange.target_stage) {
                let output_partition = match exchange.partitioning {
                    Partitioning::Single => 0,
                    Partitioning::Broadcast => 0,
                    Partitioning::Hash { .. } | Partitioning::RoundRobin { .. } => {
                        consumer.task_id.partition
                    }
                };
                locations.push(ExchangeLocation {
                    exchange_id: exchange.id.clone(),
                    producer: assignment.task_id.clone(),
                    output_partition,
                    worker_uri: self.exchange_worker_uri(consumer)?,
                });
            }
        }
        Ok(locations)
    }

    fn stage_assignments(&self, stage_id: StageId) -> impl Iterator<Item = &TaskAssignment> {
        self.assignments
            .values()
            .filter(move |assignment| assignment.task_id.stage_id == stage_id)
    }

    fn stage_task_count(&self, stage_id: StageId) -> Result<usize> {
        self.graph
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.task_count)
            .ok_or_else(|| execution_error("task references an unknown stage"))
    }

    fn exchange_worker_uri(&self, consumer: &TaskAssignment) -> Result<String> {
        if let Some(uri) = &self.exchange_store_uri {
            return Ok(uri.clone());
        }
        let worker_id = self
            .exchange_workers
            .get(&(consumer.task_id.stage_id, consumer.task_id.partition))
            .ok_or_else(|| execution_error("exchange references unknown consumer placement"))?;
        self.workers
            .iter()
            .find(|worker| &worker.node_id == worker_id)
            .map(|worker| worker.address.clone())
            .ok_or_else(|| execution_error("exchange references unknown worker"))
    }

    fn retry_worker(&self, task_id: &TaskId) -> Option<String> {
        let current = self
            .assignments
            .get(&(task_id.stage_id, task_id.partition))?;
        Some(self.next_worker(task_id.partition, &current.worker_id))
    }
}

fn ordinal(count: u32) -> String {
    let suffix = match (count % 10, count % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{count}{suffix}")
}

fn build_assignments(
    graph: &StageGraph,
    workers: &[NodeInfo],
) -> BTreeMap<(StageId, usize), TaskAssignment> {
    let mut assignments = BTreeMap::new();
    for stage in &graph.stages {
        let input_exchanges = graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.target_stage == stage.id)
            .map(|exchange| exchange.id.clone())
            .collect::<Vec<_>>();
        let output_exchanges = graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.source_stage == stage.id)
            .map(|exchange| exchange.id.clone())
            .collect::<Vec<_>>();
        for partition in 0..stage.task_count {
            let worker = &workers[partition % workers.len()];
            assignments.insert(
                (stage.id, partition),
                TaskAssignment {
                    task_id: TaskId {
                        query_id: graph.query_id.clone(),
                        stage_id: stage.id,
                        partition,
                        attempt: 0,
                    },
                    worker_id: worker.node_id.clone(),
                    splits: Vec::new(),
                    input_exchanges: input_exchanges.clone(),
                    output_exchanges: output_exchanges.clone(),
                },
            );
        }
    }
    assignments
}

fn validate_fragments(
    graph: &StageGraph,
    fragments: &BTreeMap<StageId, ExecutableFragment>,
) -> Result<()> {
    let expected = graph
        .stages
        .iter()
        .map(|stage| stage.id)
        .collect::<BTreeSet<_>>();
    let actual = fragments.keys().copied().collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(execution_error(
            "executable fragments must exactly cover the stage graph",
        ));
    }
    for (stage_id, fragment) in fragments {
        if fragment.stage_id != *stage_id {
            return Err(execution_error(
                "executable fragment key does not match its stage ID",
            ));
        }
        fragment.validate()?;
    }
    Ok(())
}

fn validate_workers(workers: &[NodeInfo]) -> Result<()> {
    if workers.is_empty() {
        return Err(execution_error(
            "distributed orchestration requires at least one worker",
        ));
    }
    let mut ids = BTreeSet::new();
    for worker in workers {
        if worker.node_id.trim().is_empty() || worker.address.trim().is_empty() {
            return Err(execution_error(
                "distributed workers require non-empty IDs and addresses",
            ));
        }
        if !ids.insert(worker.node_id.as_str()) {
            return Err(execution_error("distributed worker IDs must be unique"));
        }
    }
    Ok(())
}

fn execution_error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use kaveon_core::{
        EXECUTABLE_FRAGMENT_VERSION, ExchangeDescriptor, FragmentNode, FragmentNodeId,
        FragmentOperator, PlanNode, PlanPhase, StageFragment,
    };

    use super::*;
    use crate::cluster::NodeRole;

    fn worker(id: &str) -> NodeInfo {
        NodeInfo {
            node_id: id.into(),
            role: NodeRole::Worker,
            address: format!("http://{id}:8080"),
            version: "test".into(),
            environment: "test".into(),
            uptime_secs: 0,
            last_heartbeat: 0,
            memory_rss_bytes: 0,
            memory_allocated_bytes: 0,
            memory_limit_bytes: None,
            catalog_snapshot_id: None,
            result_cache: None,
            admission: None,
            resource_groups: None,
        }
    }

    fn graph() -> StageGraph {
        StageGraph {
            query_id: "query".into(),
            root_stage: StageId(1),
            stages: vec![stage(0, 2), stage(1, 2)],
            exchanges: vec![ExchangeDescriptor {
                id: ExchangeId("exchange-0-1".into()),
                source_stage: StageId(0),
                target_stage: StageId(1),
                partitioning: Partitioning::Hash {
                    columns: vec!["key".into()],
                    partition_count: 2,
                },
            }],
        }
    }

    fn stage(id: u32, task_count: usize) -> StageFragment {
        StageFragment {
            id: StageId(id),
            task_count,
            plan: PlanNode::new(id, PlanPhase::Physical, "test"),
        }
    }

    fn fragments() -> BTreeMap<StageId, ExecutableFragment> {
        [0, 1]
            .into_iter()
            .map(|id| {
                let stage_id = StageId(id);
                (
                    stage_id,
                    ExecutableFragment {
                        version: EXECUTABLE_FRAGMENT_VERSION,
                        stage_id,
                        root: FragmentNodeId(0),
                        nodes: vec![FragmentNode {
                            id: FragmentNodeId(0),
                            inputs: Vec::new(),
                            operator: FragmentOperator::ExchangeInput(kaveon_core::ExchangeInput {
                                exchange_id: ExchangeId(format!("input-{id}")),
                            }),
                        }],
                    },
                )
            })
            .collect()
    }

    #[test]
    fn dispatches_dependencies_in_stage_order_with_exchange_routes() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-b"), worker("worker-a")],
        )
        .unwrap();

        let sources = orchestrator.ready_dispatches().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].assignment.worker_id, "worker-a");
        assert_eq!(sources[1].assignment.worker_id, "worker-b");
        assert_eq!(
            sources[1].execution_partition,
            ExecutionPartition { index: 1, count: 2 }
        );
        assert_eq!(sources[0].exchange_outputs.len(), 2);
        for source in sources {
            orchestrator.start_task(&source.assignment.task_id).unwrap();
            orchestrator
                .finish_task(&source.assignment.task_id)
                .unwrap();
        }

        let consumers = orchestrator.ready_dispatches().unwrap();
        assert_eq!(consumers.len(), 2);
        assert_eq!(consumers[0].exchange_inputs.len(), 2);
        assert!(
            consumers[0]
                .exchange_inputs
                .iter()
                .all(|location| location.output_partition == 0)
        );
        assert!(
            consumers[1]
                .exchange_inputs
                .iter()
                .all(|location| location.output_partition == 1)
        );
    }

    #[test]
    fn retries_on_the_next_worker_and_rejects_incomplete_fragments() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        let first = orchestrator.ready_dispatches().unwrap().remove(0);
        orchestrator.start_task(&first.assignment.task_id).unwrap();
        assert!(
            orchestrator
                .fail_task(&first.assignment.task_id, "lost worker")
                .unwrap()
        );
        let retry = orchestrator
            .ready_dispatches()
            .unwrap()
            .into_iter()
            .find(|dispatch| dispatch.assignment.task_id.partition == 0)
            .unwrap();
        assert_eq!(retry.assignment.task_id.attempt, 1);
        assert_eq!(retry.assignment.worker_id, "worker-b");

        let mut missing = fragments();
        missing.remove(&StageId(1));
        assert!(CoordinatorOrchestrator::new(graph(), missing, vec![worker("worker-a")]).is_err());
    }

    #[test]
    fn consumer_retry_reads_original_exchange_placement_and_cleanup_matches() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        let producers = orchestrator.ready_dispatches().unwrap();
        for producer in producers {
            orchestrator
                .start_task(&producer.assignment.task_id)
                .unwrap();
            orchestrator
                .finish_task(&producer.assignment.task_id)
                .unwrap();
        }
        let consumers = orchestrator.ready_dispatches().unwrap();
        let first = &consumers[0];
        orchestrator.start_task(&first.assignment.task_id).unwrap();
        assert!(
            orchestrator
                .fail_task(&first.assignment.task_id, "temporary pressure")
                .unwrap()
        );
        let retry = orchestrator
            .ready_dispatches()
            .unwrap()
            .into_iter()
            .find(|dispatch| {
                dispatch.assignment.task_id.partition == first.assignment.task_id.partition
            })
            .unwrap();
        assert_ne!(retry.assignment.worker_id, first.assignment.worker_id);
        assert_eq!(retry.exchange_inputs, first.exchange_inputs);
        orchestrator.start_task(&retry.assignment.task_id).unwrap();
        orchestrator.finish_task(&retry.assignment.task_id).unwrap();
        for consumer in consumers.into_iter().skip(1) {
            orchestrator
                .start_task(&consumer.assignment.task_id)
                .unwrap();
            orchestrator
                .finish_task(&consumer.assignment.task_id)
                .unwrap();
        }
        let cleanup = orchestrator.drain_exchange_cleanup().unwrap();
        for location in retry.exchange_inputs {
            assert!(
                cleanup
                    .iter()
                    .any(|cleanup| cleanup.locations.contains(&location))
            );
        }
    }

    fn run_stage(orchestrator: &mut CoordinatorOrchestrator, stage: u32) -> Vec<TaskDispatch> {
        let dispatches = orchestrator
            .ready_dispatches()
            .unwrap()
            .into_iter()
            .filter(|dispatch| dispatch.assignment.task_id.stage_id == StageId(stage))
            .collect::<Vec<_>>();
        for dispatch in &dispatches {
            orchestrator
                .start_task(&dispatch.assignment.task_id)
                .unwrap();
            orchestrator
                .finish_task(&dispatch.assignment.task_id)
                .unwrap();
        }
        dispatches
    }

    fn retry(stage: u32, partition: usize, from: &str, to: &str, attempt: u32) -> StageRetry {
        StageRetry {
            stage,
            partition,
            from: from.into(),
            to: to.into(),
            attempt,
        }
    }

    #[test]
    fn losing_the_worker_holding_a_consumers_spool_reexecutes_its_producers() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        run_stage(&mut orchestrator, 0);
        let consumers = orchestrator.ready_dispatches().unwrap();
        for consumer in &consumers {
            orchestrator
                .start_task(&consumer.assignment.task_id)
                .unwrap();
        }
        assert_eq!(consumers[1].assignment.worker_id, "worker-b");
        assert!(
            consumers[1]
                .exchange_inputs
                .iter()
                .all(|location| location.worker_uri == "http://worker-b:8080")
        );

        // worker-b goes: the spool of consumer partition 1 goes with it.
        let recovery = orchestrator.lose_worker("worker-b").unwrap();
        assert_eq!(recovery.worker, "worker-b");
        assert_eq!(
            recovery.moved_spools,
            vec![SpoolMove {
                stage: 1,
                partition: 1,
                from: "worker-b".into(),
                to: "worker-a".into(),
            }]
        );
        assert_eq!(
            recovery.stage_retries,
            vec![
                retry(1, 1, "worker-b", "worker-a", 1),
                retry(0, 0, "worker-a", "worker-a", 1),
                retry(0, 1, "worker-b", "worker-a", 1),
            ]
        );
        assert_eq!(recovery.reexecuted_stages, vec![0]);
        assert_eq!(orchestrator.live_workers().len(), 1);
        assert!(!orchestrator.is_current_attempt(&consumers[1].assignment.task_id));
        // The consumer on worker-a holds its input: it keeps running.
        assert!(orchestrator.is_current_attempt(&consumers[0].assignment.task_id));
        // A second report of the same loss is nothing new.
        assert!(orchestrator.lose_worker("worker-b").unwrap().is_empty());

        // The producers run again first, on the survivor, as attempt 1,
        // writing partition 1 to its new home.
        let producers = orchestrator.ready_dispatches().unwrap();
        assert_eq!(producers.len(), 2);
        for producer in &producers {
            assert_eq!(producer.assignment.task_id.stage_id, StageId(0));
            assert_eq!(producer.assignment.task_id.attempt, 1);
            assert_eq!(producer.assignment.worker_id, "worker-a");
            assert!(
                producer
                    .exchange_outputs
                    .iter()
                    .all(|location| location.worker_uri == "http://worker-a:8080")
            );
        }
        run_stage(&mut orchestrator, 0);
        // Then the moved consumer, reading the new attempts where they are.
        let retried = orchestrator.ready_dispatches().unwrap();
        assert_eq!(retried.len(), 1);
        let retried = &retried[0];
        assert_eq!(retried.assignment.task_id.partition, 1);
        assert_eq!(retried.assignment.task_id.attempt, 1);
        assert_eq!(retried.assignment.worker_id, "worker-a");
        assert_eq!(retried.exchange_inputs.len(), 2);
        for location in &retried.exchange_inputs {
            assert_eq!(location.producer.attempt, 1);
            assert_eq!(location.output_partition, 1);
            assert_eq!(location.worker_uri, "http://worker-a:8080");
        }
        orchestrator
            .start_task(&retried.assignment.task_id)
            .unwrap();
        orchestrator
            .finish_task(&retried.assignment.task_id)
            .unwrap();
        orchestrator
            .finish_task(&consumers[0].assignment.task_id)
            .unwrap();
        assert!(orchestrator.is_finished());
        let cleanup = orchestrator.drain_exchange_cleanup().unwrap();
        assert_eq!(cleanup.len(), 1);
        assert!(
            cleanup[0]
                .locations
                .iter()
                .all(|location| location.producer.attempt == 1
                    && location.worker_uri == "http://worker-a:8080")
        );
    }

    #[test]
    fn losing_a_worker_while_producers_run_supersedes_every_producer_attempt() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        let producers = orchestrator.ready_dispatches().unwrap();
        for producer in &producers {
            orchestrator
                .start_task(&producer.assignment.task_id)
                .unwrap();
        }
        let recovery = orchestrator.lose_worker("worker-b").unwrap();
        // Partition 1 ran on the lost worker; partition 0 was writing to
        // it. Both run again, and the one that was on worker-a is stale
        // whatever it reports.
        assert_eq!(
            recovery.stage_retries,
            vec![
                retry(0, 1, "worker-b", "worker-a", 1),
                retry(0, 0, "worker-a", "worker-a", 1),
            ]
        );
        assert!(!orchestrator.is_current_attempt(&producers[0].assignment.task_id));
        assert!(
            orchestrator
                .finish_task(&producers[0].assignment.task_id)
                .is_err()
        );
        run_stage(&mut orchestrator, 0);
        let consumers = orchestrator.ready_dispatches().unwrap();
        assert_eq!(consumers.len(), 2);
        assert!(
            consumers
                .iter()
                .all(|consumer| consumer.assignment.worker_id == "worker-a")
        );
        for consumer in &consumers {
            assert!(
                consumer
                    .exchange_inputs
                    .iter()
                    .all(|location| location.producer.attempt == 1
                        && location.worker_uri == "http://worker-a:8080")
            );
        }
        run_stage(&mut orchestrator, 1);
        assert!(orchestrator.is_finished());
    }

    #[test]
    fn a_stage_reexecuted_beyond_the_limit_fails_the_query_naming_worker_and_stage() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b"), worker("worker-c")],
        )
        .unwrap();
        orchestrator.set_stage_retry_limit(1);
        run_stage(&mut orchestrator, 0);
        let recovery = orchestrator.lose_worker("worker-b").unwrap();
        assert_eq!(recovery.reexecuted_stages, vec![0]);
        run_stage(&mut orchestrator, 0);
        let error = orchestrator
            .lose_worker("worker-a")
            .unwrap_err()
            .to_string();
        assert!(error.contains("worker 'worker-a'"), "{error}");
        assert!(error.contains("stage 0"), "{error}");
        assert!(error.contains("KAVEON_STAGE_RETRY_LIMIT=1"), "{error}");
        assert!(orchestrator.is_terminal());
        assert!(!orchestrator.is_finished());

        // Losing the last worker fails as well.
        let mut lone =
            CoordinatorOrchestrator::new(graph(), fragments(), vec![worker("worker-a")]).unwrap();
        assert!(lone.lose_worker("worker-a").is_err());
        assert!(lone.is_terminal());
    }

    #[test]
    fn a_reexecuted_stage_whose_inputs_were_released_reexecutes_its_producers_too() {
        let mut deep = graph();
        deep.root_stage = StageId(2);
        deep.stages.push(stage(2, 1));
        deep.exchanges.push(ExchangeDescriptor {
            id: ExchangeId("exchange-1-2".into()),
            source_stage: StageId(1),
            target_stage: StageId(2),
            partitioning: Partitioning::Single,
        });
        let mut fragments = fragments();
        fragments.insert(
            StageId(2),
            ExecutableFragment {
                version: EXECUTABLE_FRAGMENT_VERSION,
                stage_id: StageId(2),
                root: FragmentNodeId(0),
                nodes: vec![FragmentNode {
                    id: FragmentNodeId(0),
                    inputs: Vec::new(),
                    operator: FragmentOperator::ExchangeInput(kaveon_core::ExchangeInput {
                        exchange_id: ExchangeId("exchange-1-2".into()),
                    }),
                }],
            },
        );
        let mut orchestrator = CoordinatorOrchestrator::new(
            deep,
            fragments,
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        run_stage(&mut orchestrator, 0);
        run_stage(&mut orchestrator, 1);
        // Stage 1 finished: its inputs from stage 0 were released.
        assert_eq!(orchestrator.drain_exchange_cleanup().unwrap().len(), 1);
        let root = orchestrator.ready_dispatches().unwrap().remove(0);
        assert_eq!(root.assignment.worker_id, "worker-a");
        orchestrator.start_task(&root.assignment.task_id).unwrap();

        let recovery = orchestrator.lose_worker("worker-a").unwrap();
        assert_eq!(recovery.reexecuted_stages, vec![1, 0]);
        assert_eq!(
            recovery
                .moved_spools
                .iter()
                .map(|moved| (moved.stage, moved.partition))
                .collect::<Vec<_>>(),
            vec![(1, 0), (2, 0)]
        );
        assert_eq!(recovery.stage_retries.len(), 5);
        assert!(
            recovery
                .stage_retries
                .iter()
                .all(|retry| retry.to == "worker-b" && retry.attempt == 1)
        );
        run_stage(&mut orchestrator, 0);
        // Stage 1's inputs are cleaned again once it finishes again.
        run_stage(&mut orchestrator, 1);
        let cleanup = orchestrator.drain_exchange_cleanup().unwrap();
        assert_eq!(cleanup.len(), 1);
        assert_eq!(cleanup[0].exchange_id, ExchangeId("exchange-0-1".into()));
        assert!(
            cleanup[0]
                .locations
                .iter()
                .all(|location| location.producer.attempt == 1
                    && location.worker_uri == "http://worker-b:8080")
        );
        let root = orchestrator.ready_dispatches().unwrap().remove(0);
        assert_eq!(root.assignment.task_id.attempt, 1);
        assert_eq!(root.assignment.worker_id, "worker-b");
        assert!(
            root.exchange_inputs
                .iter()
                .all(|location| location.producer.attempt == 1)
        );
        orchestrator.start_task(&root.assignment.task_id).unwrap();
        orchestrator.finish_task(&root.assignment.task_id).unwrap();
        assert!(orchestrator.is_finished());
    }

    #[test]
    fn a_coordinator_relayed_exchange_loses_no_spool_with_a_worker() {
        let mut orchestrator = CoordinatorOrchestrator::new(
            graph(),
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();
        orchestrator.set_exchange_store_uri("http://coordinator:8080".into());
        run_stage(&mut orchestrator, 0);
        let consumers = orchestrator.ready_dispatches().unwrap();
        for consumer in &consumers {
            orchestrator
                .start_task(&consumer.assignment.task_id)
                .unwrap();
        }
        let recovery = orchestrator.lose_worker("worker-b").unwrap();
        assert!(recovery.moved_spools.is_empty());
        assert!(recovery.reexecuted_stages.is_empty());
        assert_eq!(
            recovery.stage_retries,
            vec![retry(1, 1, "worker-b", "worker-a", 1)]
        );
    }

    #[test]
    fn broadcast_exchange_uses_one_partition_on_every_consumer() {
        let mut broadcast = graph();
        broadcast.exchanges[0].partitioning = Partitioning::Broadcast;
        let orchestrator = CoordinatorOrchestrator::new(
            broadcast,
            fragments(),
            vec![worker("worker-a"), worker("worker-b")],
        )
        .unwrap();

        let source = orchestrator.ready_dispatches().unwrap().remove(0);
        assert_eq!(source.exchange_outputs.len(), 2);
        assert!(
            source
                .exchange_outputs
                .iter()
                .all(|location| location.output_partition == 0)
        );
        assert_ne!(
            source.exchange_outputs[0].worker_uri,
            source.exchange_outputs[1].worker_uri
        );
    }
}
