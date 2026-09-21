use std::collections::{BTreeMap, BTreeSet};

use kaveon_core::{
    ExchangeId, KaveonError, Result, StageGraph, StageId, TaskAssignment, TaskId, TaskState,
    TaskStatus,
};

const DEFAULT_MAX_TASK_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Running,
    Finished,
    Failed,
    Canceled,
}

impl StageState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::Canceled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeCleanupIntent {
    pub exchange_id: ExchangeId,
}

#[derive(Clone, Debug)]
struct TaskSlot {
    assignment: TaskAssignment,
    status: TaskStatus,
    /// Attempts of this slot that failed on their own; a re-execution after
    /// a worker loss takes an attempt number but not a failure.
    failures: u32,
}

/// What a superseded slot was doing when a worker loss replaced its attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupersededState {
    /// Not yet started: only its worker changed, the attempt stays.
    Pending,
    /// In flight: the attempt in flight is stale and its outcome is ignored.
    Running,
    /// Done, but its output must be produced again.
    Finished,
}

#[derive(Debug)]
pub struct StageRuntime {
    graph: StageGraph,
    stages: BTreeMap<StageId, StageState>,
    tasks: BTreeMap<(StageId, usize), TaskSlot>,
    task_history: Vec<TaskStatus>,
    max_task_attempts: u32,
    cleanup_ready: Vec<ExchangeCleanupIntent>,
    cleanup_emitted: BTreeSet<ExchangeId>,
}

impl StageRuntime {
    pub fn new(graph: StageGraph, assignments: Vec<TaskAssignment>) -> Result<Self> {
        Self::with_max_task_attempts(graph, assignments, DEFAULT_MAX_TASK_ATTEMPTS)
    }

    pub fn with_max_task_attempts(
        graph: StageGraph,
        assignments: Vec<TaskAssignment>,
        max_task_attempts: u32,
    ) -> Result<Self> {
        graph.validate()?;
        if max_task_attempts == 0 {
            return Err(KaveonError::Execution(
                "stage runtime requires at least one task attempt".into(),
            ));
        }
        let task_counts = graph
            .stages
            .iter()
            .map(|stage| (stage.id, stage.task_count))
            .collect::<BTreeMap<_, _>>();
        let mut tasks = BTreeMap::new();
        for assignment in assignments {
            validate_assignment(&graph, &task_counts, &assignment)?;
            let key = (assignment.task_id.stage_id, assignment.task_id.partition);
            if tasks.contains_key(&key) {
                return Err(KaveonError::Execution(format!(
                    "duplicate task assignment for stage {} partition {}",
                    key.0.0, key.1
                )));
            }
            let status = TaskStatus {
                task_id: assignment.task_id.clone(),
                state: TaskState::Pending,
                worker_id: None,
                failure: None,
            };
            tasks.insert(
                key,
                TaskSlot {
                    assignment,
                    status,
                    failures: 0,
                },
            );
        }
        for (stage_id, task_count) in &task_counts {
            let assigned = tasks
                .keys()
                .filter(|(assigned_stage, _)| assigned_stage == stage_id)
                .count();
            if assigned != *task_count {
                return Err(KaveonError::Execution(format!(
                    "stage {} requires {} task assignments but received {}",
                    stage_id.0, task_count, assigned
                )));
            }
        }
        let stages = task_counts
            .keys()
            .map(|stage_id| (*stage_id, StageState::Pending))
            .collect();
        Ok(Self {
            graph,
            stages,
            tasks,
            task_history: Vec::new(),
            max_task_attempts,
            cleanup_ready: Vec::new(),
            cleanup_emitted: BTreeSet::new(),
        })
    }

    pub fn ready_tasks(&self) -> Vec<TaskAssignment> {
        self.tasks
            .values()
            .filter(|slot| {
                slot.status.state == TaskState::Pending
                    && self.stage_is_ready(slot.assignment.task_id.stage_id)
            })
            .map(|slot| slot.assignment.clone())
            .collect()
    }

    pub fn start_task(&mut self, task_id: &TaskId) -> Result<()> {
        let key = task_key(task_id);
        if !self.stage_is_ready(task_id.stage_id) {
            return Err(KaveonError::Execution(format!(
                "stage {} dependencies are not complete",
                task_id.stage_id.0
            )));
        }
        let slot = self.current_slot_mut(key, task_id)?;
        require_task_state(&slot.status, TaskState::Pending, "start")?;
        slot.status.state = TaskState::Running;
        slot.status.worker_id = Some(slot.assignment.worker_id.clone());
        self.stages.insert(task_id.stage_id, StageState::Running);
        Ok(())
    }

    pub fn finish_task(&mut self, task_id: &TaskId) -> Result<()> {
        let key = task_key(task_id);
        let slot = self.current_slot_mut(key, task_id)?;
        require_task_state(&slot.status, TaskState::Running, "finish")?;
        slot.status.state = TaskState::Finished;
        if self.stage_tasks_are(task_id.stage_id, TaskState::Finished) {
            self.stages.insert(task_id.stage_id, StageState::Finished);
            self.queue_consumed_exchange_cleanup(task_id.stage_id);
        }
        Ok(())
    }

    pub fn fail_task(
        &mut self,
        task_id: &TaskId,
        failure: impl Into<String>,
        retry_worker: Option<String>,
    ) -> Result<Option<TaskAssignment>> {
        let key = task_key(task_id);
        let failure = failure.into();
        let max_task_attempts = self.max_task_attempts;
        let next_attempt = task_id.attempt.saturating_add(1);
        let (failed_status, retry) = {
            let slot = self.current_slot_mut(key, task_id)?;
            require_task_state(&slot.status, TaskState::Running, "fail")?;
            slot.status.state = TaskState::Failed;
            slot.status.failure = Some(failure);
            slot.failures = slot.failures.saturating_add(1);
            let failed_status = slot.status.clone();
            let retry = if slot.failures < max_task_attempts {
                let mut retry = slot.assignment.clone();
                retry.task_id.attempt = next_attempt;
                if let Some(worker) = retry_worker {
                    retry.worker_id = worker;
                }
                slot.assignment = retry.clone();
                slot.status = TaskStatus {
                    task_id: retry.task_id.clone(),
                    state: TaskState::Pending,
                    worker_id: None,
                    failure: None,
                };
                Some(retry)
            } else {
                None
            };
            (failed_status, retry)
        };
        self.task_history.push(failed_status);
        if let Some(retry) = retry {
            return Ok(Some(retry));
        }
        self.stages.insert(task_id.stage_id, StageState::Failed);
        self.cancel_unfinished_tasks();
        self.queue_all_exchange_cleanup();
        Ok(None)
    }

    pub fn cancel(&mut self) {
        self.cancel_unfinished_tasks();
        for state in self.stages.values_mut() {
            if !state.is_terminal() {
                *state = StageState::Canceled;
            }
        }
        self.queue_all_exchange_cleanup();
    }

    /// Replace a slot's attempt after a worker loss: a finished or running
    /// attempt is superseded by a pending one on `worker` with the next
    /// attempt number (a running one is left to its fate; its outcome is
    /// stale), a pending one only moves to `worker`. The slot's stage is
    /// open again, and if it had finished, the cleanup of its input
    /// exchanges may be emitted again once it finishes anew. Returns the
    /// state the slot was in and the assignment now current.
    pub fn supersede_task(
        &mut self,
        stage_id: StageId,
        partition: usize,
        worker: String,
        reason: &str,
    ) -> Result<(SupersededState, TaskAssignment)> {
        let stage_was_finished = self.stage_state(stage_id) == Some(StageState::Finished);
        let slot = self.tasks.get_mut(&(stage_id, partition)).ok_or_else(|| {
            KaveonError::Execution(format!(
                "unknown task slot: stage {} partition {partition}",
                stage_id.0
            ))
        })?;
        let was = match slot.status.state {
            TaskState::Pending => SupersededState::Pending,
            TaskState::Running => SupersededState::Running,
            TaskState::Finished => SupersededState::Finished,
            TaskState::Failed | TaskState::Canceled => {
                return Err(KaveonError::Execution(format!(
                    "cannot supersede task {} in state {:?}",
                    slot.status.task_id, slot.status.state
                )));
            }
        };
        let mut next = slot.assignment.clone();
        next.worker_id = worker;
        if was != SupersededState::Pending {
            next.task_id.attempt = next.task_id.attempt.saturating_add(1);
            let mut superseded = slot.status.clone();
            superseded.state = TaskState::Failed;
            superseded.failure = Some(reason.to_owned());
            self.task_history.push(superseded);
        }
        slot.assignment = next.clone();
        slot.status = TaskStatus {
            task_id: next.task_id.clone(),
            state: TaskState::Pending,
            worker_id: None,
            failure: None,
        };
        self.stages.insert(stage_id, StageState::Running);
        if stage_was_finished {
            let inputs = self
                .graph
                .exchanges
                .iter()
                .filter(|exchange| exchange.target_stage == stage_id)
                .map(|exchange| exchange.id.clone())
                .collect::<Vec<_>>();
            for exchange_id in inputs {
                self.cleanup_emitted.remove(&exchange_id);
            }
        }
        Ok((was, next))
    }

    /// Whether `task_id` is the attempt the slot currently stands on; an
    /// older attempt's outcome is stale and must be ignored.
    pub fn is_current_attempt(&self, task_id: &TaskId) -> bool {
        self.tasks
            .get(&task_key(task_id))
            .is_some_and(|slot| slot.status.task_id == *task_id)
    }

    /// The assignment each slot currently stands on, with its state.
    pub fn slots(&self) -> impl Iterator<Item = (&TaskAssignment, TaskState)> {
        self.tasks
            .values()
            .map(|slot| (&slot.assignment, slot.status.state))
    }

    pub fn stage_state(&self, stage_id: StageId) -> Option<StageState> {
        self.stages.get(&stage_id).copied()
    }

    pub fn task_status(&self, stage_id: StageId, partition: usize) -> Option<&TaskStatus> {
        self.tasks
            .get(&(stage_id, partition))
            .map(|slot| &slot.status)
    }

    pub fn task_history(&self) -> &[TaskStatus] {
        &self.task_history
    }

    pub fn is_finished(&self) -> bool {
        self.stage_state(self.graph.root_stage) == Some(StageState::Finished)
    }

    pub fn is_terminal(&self) -> bool {
        self.stages.values().all(|state| state.is_terminal())
    }

    pub fn drain_cleanup_intents(&mut self) -> Vec<ExchangeCleanupIntent> {
        std::mem::take(&mut self.cleanup_ready)
    }

    fn current_slot_mut(
        &mut self,
        key: (StageId, usize),
        task_id: &TaskId,
    ) -> Result<&mut TaskSlot> {
        let slot = self
            .tasks
            .get_mut(&key)
            .ok_or_else(|| KaveonError::Execution(format!("unknown task assignment {task_id}")))?;
        if slot.status.task_id != *task_id {
            return Err(KaveonError::Execution(format!(
                "task attempt {} is stale; current attempt is {}",
                task_id.attempt, slot.status.task_id.attempt
            )));
        }
        Ok(slot)
    }

    fn stage_is_ready(&self, stage_id: StageId) -> bool {
        self.stages
            .get(&stage_id)
            .is_some_and(|state| matches!(state, StageState::Pending | StageState::Running))
            && self
                .graph
                .exchanges
                .iter()
                .filter(|exchange| exchange.target_stage == stage_id)
                .all(|exchange| {
                    self.stage_state(exchange.source_stage) == Some(StageState::Finished)
                })
    }

    fn stage_tasks_are(&self, stage_id: StageId, state: TaskState) -> bool {
        self.tasks
            .values()
            .all(|slot| slot.assignment.task_id.stage_id != stage_id || slot.status.state == state)
    }

    fn cancel_unfinished_tasks(&mut self) {
        for slot in self.tasks.values_mut() {
            if matches!(slot.status.state, TaskState::Pending | TaskState::Running) {
                slot.status.state = TaskState::Canceled;
            }
        }
        for state in self.stages.values_mut() {
            if !state.is_terminal() {
                *state = StageState::Canceled;
            }
        }
    }

    fn queue_consumed_exchange_cleanup(&mut self, target_stage: StageId) {
        let exchange_ids = self
            .graph
            .exchanges
            .iter()
            .filter(|exchange| exchange.target_stage == target_stage)
            .map(|exchange| exchange.id.clone())
            .collect::<Vec<_>>();
        self.queue_exchange_cleanup(exchange_ids);
    }

    fn queue_all_exchange_cleanup(&mut self) {
        let exchange_ids = self
            .graph
            .exchanges
            .iter()
            .map(|exchange| exchange.id.clone())
            .collect::<Vec<_>>();
        self.queue_exchange_cleanup(exchange_ids);
    }

    fn queue_exchange_cleanup(&mut self, exchange_ids: Vec<ExchangeId>) {
        for exchange_id in exchange_ids {
            if self.cleanup_emitted.insert(exchange_id.clone()) {
                self.cleanup_ready
                    .push(ExchangeCleanupIntent { exchange_id });
            }
        }
    }
}

fn validate_assignment(
    graph: &StageGraph,
    task_counts: &BTreeMap<StageId, usize>,
    assignment: &TaskAssignment,
) -> Result<()> {
    if assignment.task_id.query_id != graph.query_id {
        return Err(KaveonError::Execution(format!(
            "task query '{}' does not match stage graph query '{}'",
            assignment.task_id.query_id, graph.query_id
        )));
    }
    let task_count = task_counts
        .get(&assignment.task_id.stage_id)
        .ok_or_else(|| {
            KaveonError::Execution(format!(
                "task references unknown stage {}",
                assignment.task_id.stage_id.0
            ))
        })?;
    if assignment.task_id.partition >= *task_count {
        return Err(KaveonError::Execution(format!(
            "task partition {} exceeds stage {} task count {}",
            assignment.task_id.partition, assignment.task_id.stage_id.0, task_count
        )));
    }
    if assignment.task_id.attempt != 0 {
        return Err(KaveonError::Execution(
            "initial task assignments must use attempt zero".into(),
        ));
    }
    Ok(())
}

fn require_task_state(status: &TaskStatus, expected: TaskState, action: &str) -> Result<()> {
    if status.state != expected {
        return Err(KaveonError::Execution(format!(
            "cannot {action} task {} in state {:?}",
            status.task_id, status.state
        )));
    }
    Ok(())
}

fn task_key(task_id: &TaskId) -> (StageId, usize) {
    (task_id.stage_id, task_id.partition)
}

#[cfg(test)]
mod tests {
    use kaveon_core::{
        ExchangeDescriptor, Partitioning, PlanNode, PlanPhase, SplitDescriptor, SplitId,
        StageFragment,
    };

    use super::*;

    fn graph() -> StageGraph {
        StageGraph {
            query_id: "query".into(),
            root_stage: StageId(1),
            stages: vec![fragment(0, 2), fragment(1, 1)],
            exchanges: vec![ExchangeDescriptor {
                id: ExchangeId("exchange-0-1".into()),
                source_stage: StageId(0),
                target_stage: StageId(1),
                partitioning: Partitioning::Single,
            }],
        }
    }

    fn fragment(id: u32, task_count: usize) -> StageFragment {
        StageFragment {
            id: StageId(id),
            task_count,
            plan: PlanNode::new(id, PlanPhase::Physical, "test"),
        }
    }

    fn assignments() -> Vec<TaskAssignment> {
        vec![assignment(0, 0), assignment(0, 1), assignment(1, 0)]
    }

    fn assignment(stage: u32, partition: usize) -> TaskAssignment {
        TaskAssignment {
            task_id: TaskId {
                query_id: "query".into(),
                stage_id: StageId(stage),
                partition,
                attempt: 0,
            },
            worker_id: format!("worker-{partition}"),
            splits: vec![SplitDescriptor {
                id: SplitId(format!("split-{stage}-{partition}")),
                source_uri: "file:///data.parquet".into(),
                properties: BTreeMap::new(),
            }],
            input_exchanges: if stage == 1 {
                vec![ExchangeId("exchange-0-1".into())]
            } else {
                Vec::new()
            },
            output_exchanges: if stage == 0 {
                vec![ExchangeId("exchange-0-1".into())]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn releases_tasks_only_after_dependencies_finish() {
        let mut runtime = StageRuntime::new(graph(), assignments()).unwrap();
        let first = runtime.ready_tasks();
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|task| task.task_id.stage_id == StageId(0)));

        for task in first {
            runtime.start_task(&task.task_id).unwrap();
            runtime.finish_task(&task.task_id).unwrap();
        }
        assert_eq!(runtime.stage_state(StageId(0)), Some(StageState::Finished));
        let final_tasks = runtime.ready_tasks();
        assert_eq!(final_tasks.len(), 1);
        assert_eq!(final_tasks[0].task_id.stage_id, StageId(1));
    }

    #[test]
    fn retries_increment_attempt_and_reject_stale_updates() {
        let mut runtime = StageRuntime::new(graph(), assignments()).unwrap();
        let task = runtime.ready_tasks().remove(0);
        runtime.start_task(&task.task_id).unwrap();
        let retry = runtime
            .fail_task(&task.task_id, "worker lost", Some("worker-retry".into()))
            .unwrap()
            .unwrap();

        assert_eq!(retry.task_id.attempt, 1);
        assert_eq!(retry.worker_id, "worker-retry");
        assert!(runtime.start_task(&task.task_id).is_err());
        runtime.start_task(&retry.task_id).unwrap();
        assert_eq!(runtime.task_history().len(), 1);
        assert_eq!(
            runtime
                .task_status(StageId(0), retry.task_id.partition)
                .unwrap()
                .state,
            TaskState::Running
        );
    }

    #[test]
    fn exhausted_retry_fails_stage_and_cancels_remaining_work() {
        let mut runtime = StageRuntime::with_max_task_attempts(graph(), assignments(), 1).unwrap();
        let task = runtime.ready_tasks().remove(0);
        runtime.start_task(&task.task_id).unwrap();
        assert!(
            runtime
                .fail_task(&task.task_id, "fatal", None)
                .unwrap()
                .is_none()
        );

        assert_eq!(runtime.stage_state(StageId(0)), Some(StageState::Failed));
        assert_eq!(runtime.stage_state(StageId(1)), Some(StageState::Canceled));
        assert!(runtime.is_terminal());
        assert_eq!(runtime.drain_cleanup_intents().len(), 1);
    }

    #[test]
    fn completing_consumer_emits_cleanup_once() {
        let mut runtime = StageRuntime::new(graph(), assignments()).unwrap();
        for task in runtime.ready_tasks() {
            runtime.start_task(&task.task_id).unwrap();
            runtime.finish_task(&task.task_id).unwrap();
        }
        let final_task = runtime.ready_tasks().remove(0);
        runtime.start_task(&final_task.task_id).unwrap();
        runtime.finish_task(&final_task.task_id).unwrap();

        assert!(runtime.is_finished());
        assert_eq!(
            runtime.drain_cleanup_intents(),
            vec![ExchangeCleanupIntent {
                exchange_id: ExchangeId("exchange-0-1".into())
            }]
        );
        runtime.cancel();
        assert!(runtime.drain_cleanup_intents().is_empty());
    }

    #[test]
    fn a_superseded_attempt_takes_a_number_but_not_a_failure() {
        let mut runtime = StageRuntime::with_max_task_attempts(graph(), assignments(), 2).unwrap();
        for task in runtime.ready_tasks() {
            runtime.start_task(&task.task_id).unwrap();
            runtime.finish_task(&task.task_id).unwrap();
        }
        assert_eq!(runtime.stage_state(StageId(0)), Some(StageState::Finished));
        let root = runtime.ready_tasks().remove(0);
        runtime.start_task(&root.task_id).unwrap();

        // The finished producer runs again as attempt 1; the running root
        // is superseded by attempt 1 and its own outcome is stale.
        let (was, next) = runtime
            .supersede_task(StageId(0), 1, "worker-x".into(), "worker lost")
            .unwrap();
        assert_eq!(was, SupersededState::Finished);
        assert_eq!(next.task_id.attempt, 1);
        assert_eq!(next.worker_id, "worker-x");
        assert_eq!(runtime.stage_state(StageId(0)), Some(StageState::Running));
        assert!(
            runtime
                .ready_tasks()
                .iter()
                .any(|task| task.task_id == next.task_id)
        );
        let (was, root_next) = runtime
            .supersede_task(StageId(1), 0, "worker-x".into(), "worker lost")
            .unwrap();
        assert_eq!(was, SupersededState::Running);
        assert!(!runtime.is_current_attempt(&root.task_id));
        assert!(runtime.is_current_attempt(&root_next.task_id));
        assert!(runtime.finish_task(&root.task_id).is_err());
        assert_eq!(runtime.task_history().len(), 2);
        // A pending slot only moves.
        let (was, moved) = runtime
            .supersede_task(StageId(1), 0, "worker-y".into(), "worker lost")
            .unwrap();
        assert_eq!(was, SupersededState::Pending);
        assert_eq!(moved.task_id.attempt, 1);
        assert_eq!(moved.worker_id, "worker-y");

        // Attempt 1 of the producer still has its full failure budget: it
        // may fail once (attempt 2) before the stage fails.
        runtime.start_task(&next.task_id).unwrap();
        let retry = runtime
            .fail_task(&next.task_id, "pressure", None)
            .unwrap()
            .expect("one failure is within the budget of two");
        assert_eq!(retry.task_id.attempt, 2);
        runtime.start_task(&retry.task_id).unwrap();
        assert!(
            runtime
                .fail_task(&retry.task_id, "pressure", None)
                .unwrap()
                .is_none()
        );
        assert_eq!(runtime.stage_state(StageId(0)), Some(StageState::Failed));
    }

    #[test]
    fn a_reopened_stage_emits_its_input_cleanup_again_when_it_finishes_again() {
        let mut runtime = StageRuntime::new(graph(), assignments()).unwrap();
        for task in runtime.ready_tasks() {
            runtime.start_task(&task.task_id).unwrap();
            runtime.finish_task(&task.task_id).unwrap();
        }
        let root = runtime.ready_tasks().remove(0);
        runtime.start_task(&root.task_id).unwrap();
        runtime.finish_task(&root.task_id).unwrap();
        assert_eq!(runtime.drain_cleanup_intents().len(), 1);
        let (_, next) = runtime
            .supersede_task(StageId(1), 0, "worker-x".into(), "worker lost")
            .unwrap();
        assert!(!runtime.is_finished());
        runtime.start_task(&next.task_id).unwrap();
        runtime.finish_task(&next.task_id).unwrap();
        assert!(runtime.is_finished());
        assert_eq!(runtime.drain_cleanup_intents().len(), 1);
    }

    #[test]
    fn validates_assignment_coverage_and_identity() {
        let mut missing = assignments();
        missing.pop();
        assert!(StageRuntime::new(graph(), missing).is_err());

        let mut wrong_query = assignments();
        wrong_query[0].task_id.query_id = "other".into();
        assert!(StageRuntime::new(graph(), wrong_query).is_err());
    }
}
