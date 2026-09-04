use std::collections::{BTreeMap, BTreeSet};

use kaveon_core::{
    ExchangeId, ExecutableFragment, KaveonError, Partitioning, Result, StageGraph, StageId,
    TaskAssignment, TaskId,
};

use crate::cluster::NodeInfo;
use crate::runtime::{ExchangeCleanupIntent, StageRuntime};

const DEFAULT_MAX_TASK_ATTEMPTS: u32 = 3;

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
    runtime: StageRuntime,
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
        Ok(Self {
            graph,
            fragments,
            workers,
            assignments,
            runtime,
        })
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
                            worker_uri: assignment_worker_uri(consumer, &self.workers)?,
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
                    worker_uri: assignment_worker_uri(assignment, &self.workers)?,
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
                    worker_uri: assignment_worker_uri(consumer, &self.workers)?,
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

    fn retry_worker(&self, task_id: &TaskId) -> Option<String> {
        let current = self
            .assignments
            .get(&(task_id.stage_id, task_id.partition))?;
        let current_index = self
            .workers
            .iter()
            .position(|worker| worker.node_id == current.worker_id)?;
        let next_index = (current_index + 1) % self.workers.len();
        Some(self.workers[next_index].node_id.clone())
    }
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

fn assignment_worker_uri(assignment: &TaskAssignment, workers: &[NodeInfo]) -> Result<String> {
    workers
        .iter()
        .find(|worker| worker.node_id == assignment.worker_id)
        .map(|worker| worker.address.clone())
        .ok_or_else(|| execution_error("task assignment references an unknown worker"))
}

fn execution_error(message: &str) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use kaveon_core::{
        ExchangeDescriptor, FragmentNode, FragmentNodeId, FragmentOperator, PlanNode, PlanPhase,
        StageFragment,
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
                        version: 1,
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
