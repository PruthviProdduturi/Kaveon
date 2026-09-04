use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kaveon_core::TaskId;
use tokio::sync::Notify;

const DEFAULT_MAX_TASKS: usize = 4_096;
const DEFAULT_MAX_QUERIES: usize = 1_024;
const OWNER_DROPPED_MESSAGE: &str = "task owner dropped before publishing a terminal outcome";

#[derive(Debug, PartialEq, Eq)]
pub enum TaskOutcome<T> {
    Success(Arc<T>),
    Failed(Arc<str>),
}

impl<T> Clone for TaskOutcome<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Success(value) => Self::Success(Arc::clone(value)),
            Self::Failed(message) => Self::Failed(Arc::clone(message)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    InvalidCapacity,
    TaskCapacityExceeded,
    QueryCapacityExceeded,
    RegistryPoisoned,
    TaskAlreadyCompleted,
}

impl Display for LifecycleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCapacity => "lifecycle registry capacity must be greater than zero",
            Self::TaskCapacityExceeded => "worker task registry capacity exceeded",
            Self::QueryCapacityExceeded => "query cancellation registry capacity exceeded",
            Self::RegistryPoisoned => "worker lifecycle registry lock poisoned",
            Self::TaskAlreadyCompleted => "worker task already has a terminal outcome",
        })
    }
}

impl std::error::Error for LifecycleError {}

struct TaskEntry<T> {
    outcome: Mutex<Option<TaskOutcome<T>>>,
    terminal: Notify,
}

impl<T> TaskEntry<T> {
    fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
            terminal: Notify::new(),
        }
    }

    fn outcome(&self) -> Result<Option<TaskOutcome<T>>, LifecycleError> {
        self.outcome
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)
            .map(|outcome| outcome.clone())
    }

    fn complete(&self, outcome: TaskOutcome<T>) -> Result<(), LifecycleError> {
        let mut current = self
            .outcome
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?;
        if current.is_some() {
            return Err(LifecycleError::TaskAlreadyCompleted);
        }
        *current = Some(outcome);
        drop(current);
        self.terminal.notify_waiters();
        Ok(())
    }
}

pub enum TaskClaim<T> {
    Owner(TaskOwner<T>),
    Waiter(TaskWaiter<T>),
    Completed(TaskOutcome<T>),
}

pub struct TaskOwner<T> {
    entry: Arc<TaskEntry<T>>,
    completed: bool,
}

impl<T> TaskOwner<T> {
    pub fn complete(mut self, outcome: TaskOutcome<T>) -> Result<(), LifecycleError> {
        self.entry.complete(outcome)?;
        self.completed = true;
        Ok(())
    }
}

impl<T> Drop for TaskOwner<T> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self
                .entry
                .complete(TaskOutcome::Failed(Arc::from(OWNER_DROPPED_MESSAGE)));
        }
    }
}

pub struct TaskWaiter<T> {
    entry: Arc<TaskEntry<T>>,
}

impl<T> TaskWaiter<T> {
    pub async fn wait(self) -> Result<TaskOutcome<T>, LifecycleError> {
        loop {
            let notified = self.entry.terminal.notified();
            if let Some(outcome) = self.entry.outcome()? {
                return Ok(outcome);
            }
            notified.await;
        }
    }
}

pub struct TaskRegistry<T> {
    tasks: Mutex<HashMap<TaskId, Arc<TaskEntry<T>>>>,
    max_tasks: usize,
}

impl<T> Default for TaskRegistry<T> {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            max_tasks: DEFAULT_MAX_TASKS,
        }
    }
}

impl<T> TaskRegistry<T> {
    pub fn with_capacity(max_tasks: usize) -> Result<Self, LifecycleError> {
        if max_tasks == 0 {
            return Err(LifecycleError::InvalidCapacity);
        }
        Ok(Self {
            tasks: Mutex::new(HashMap::new()),
            max_tasks,
        })
    }

    pub fn claim(&self, task_id: TaskId) -> Result<TaskClaim<T>, LifecycleError> {
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?;
        if let Some(entry) = tasks.get(&task_id) {
            return match entry.outcome()? {
                Some(outcome) => Ok(TaskClaim::Completed(outcome)),
                None => Ok(TaskClaim::Waiter(TaskWaiter {
                    entry: Arc::clone(entry),
                })),
            };
        }
        if tasks.len() >= self.max_tasks {
            return Err(LifecycleError::TaskCapacityExceeded);
        }
        let entry = Arc::new(TaskEntry::new());
        tasks.insert(task_id, Arc::clone(&entry));
        Ok(TaskClaim::Owner(TaskOwner {
            entry,
            completed: false,
        }))
    }

    pub fn remove_query(&self, query_id: &str) -> Result<usize, LifecycleError> {
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?;
        let previous = tasks.len();
        tasks.retain(|task_id, _| task_id.query_id != query_id);
        Ok(previous - tasks.len())
    }

    pub fn len(&self) -> Result<usize, LifecycleError> {
        self.tasks
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)
            .map(|tasks| tasks.len())
    }

    pub fn is_empty(&self) -> Result<bool, LifecycleError> {
        self.len().map(|length| length == 0)
    }
}

struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

#[derive(Clone)]
pub struct CancellationToken {
    state: Arc<CancellationState>,
}

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.state.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    fn cancel(&self) {
        if !self.state.cancelled.swap(true, Ordering::AcqRel) {
            self.state.notify.notify_waiters();
        }
    }
}

pub struct CancellationRegistry {
    queries: Mutex<HashMap<String, CancellationToken>>,
    max_queries: usize,
}

impl Default for CancellationRegistry {
    fn default() -> Self {
        Self {
            queries: Mutex::new(HashMap::new()),
            max_queries: DEFAULT_MAX_QUERIES,
        }
    }
}

impl CancellationRegistry {
    pub fn with_capacity(max_queries: usize) -> Result<Self, LifecycleError> {
        if max_queries == 0 {
            return Err(LifecycleError::InvalidCapacity);
        }
        Ok(Self {
            queries: Mutex::new(HashMap::new()),
            max_queries,
        })
    }

    pub fn token(&self, query_id: &str) -> Result<CancellationToken, LifecycleError> {
        let mut queries = self
            .queries
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?;
        if let Some(token) = queries.get(query_id) {
            return Ok(token.clone());
        }
        if queries.len() >= self.max_queries {
            return Err(LifecycleError::QueryCapacityExceeded);
        }
        let token = CancellationToken {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        };
        queries.insert(query_id.to_owned(), token.clone());
        Ok(token)
    }

    pub fn cancel(&self, query_id: &str) -> Result<bool, LifecycleError> {
        let token = self
            .queries
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?
            .get(query_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn remove(&self, query_id: &str) -> Result<bool, LifecycleError> {
        Ok(self
            .queries
            .lock()
            .map_err(|_| LifecycleError::RegistryPoisoned)?
            .remove(query_id)
            .is_some())
    }
}

pub struct WorkerLifecycle<T> {
    pub tasks: TaskRegistry<T>,
    pub cancellations: CancellationRegistry,
}

impl<T> Default for WorkerLifecycle<T> {
    fn default() -> Self {
        Self {
            tasks: TaskRegistry::default(),
            cancellations: CancellationRegistry::default(),
        }
    }
}

impl<T> WorkerLifecycle<T> {
    pub fn finish_query(&self, query_id: &str) -> Result<usize, LifecycleError> {
        let removed_tasks = self.tasks.remove_query(query_id)?;
        self.cancellations.remove(query_id)?;
        Ok(removed_tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaveon_core::StageId;

    fn task(query_id: &str, attempt: u32) -> TaskId {
        TaskId {
            query_id: query_id.into(),
            stage_id: StageId(2),
            partition: 3,
            attempt,
        }
    }

    #[tokio::test]
    async fn duplicate_waiter_observes_owner_outcome_and_retry_replays_it() {
        let registry = TaskRegistry::with_capacity(2).unwrap();
        let TaskClaim::Owner(owner) = registry.claim(task("query-a", 0)).unwrap() else {
            panic!("first claimant must own the task");
        };
        let TaskClaim::Waiter(waiter) = registry.claim(task("query-a", 0)).unwrap() else {
            panic!("concurrent duplicate must wait");
        };
        owner
            .complete(TaskOutcome::Success(Arc::new(vec![1_u8, 2, 3])))
            .unwrap();
        assert_eq!(
            waiter.wait().await.unwrap(),
            TaskOutcome::Success(Arc::new(vec![1, 2, 3]))
        );
        assert!(matches!(
            registry.claim(task("query-a", 0)).unwrap(),
            TaskClaim::Completed(TaskOutcome::Success(value)) if value.as_ref() == &[1, 2, 3]
        ));
    }

    #[tokio::test]
    async fn dropped_owner_releases_waiters_with_failure() {
        let registry = TaskRegistry::<Vec<u8>>::default();
        let TaskClaim::Owner(owner) = registry.claim(task("query-a", 0)).unwrap() else {
            panic!("first claimant must own the task");
        };
        let TaskClaim::Waiter(waiter) = registry.claim(task("query-a", 0)).unwrap() else {
            panic!("concurrent duplicate must wait");
        };
        drop(owner);
        assert!(matches!(
            waiter.wait().await.unwrap(),
            TaskOutcome::Failed(message) if message.as_ref() == OWNER_DROPPED_MESSAGE
        ));
    }

    #[test]
    fn task_registry_is_bounded_and_terminal_cleanup_releases_capacity() {
        let lifecycle = WorkerLifecycle::<Vec<u8>> {
            tasks: TaskRegistry::with_capacity(1).unwrap(),
            cancellations: CancellationRegistry::with_capacity(1).unwrap(),
        };
        assert!(matches!(
            lifecycle.tasks.claim(task("query-a", 0)).unwrap(),
            TaskClaim::Owner(_)
        ));
        assert_eq!(
            lifecycle.tasks.claim(task("query-b", 0)).err(),
            Some(LifecycleError::TaskCapacityExceeded)
        );
        assert_eq!(lifecycle.finish_query("query-a").unwrap(), 1);
        assert!(matches!(
            lifecycle.tasks.claim(task("query-b", 0)).unwrap(),
            TaskClaim::Owner(_)
        ));
    }

    #[test]
    fn sequential_terminal_queries_can_exceed_registry_capacity() {
        let lifecycle = WorkerLifecycle::<Vec<u8>> {
            tasks: TaskRegistry::with_capacity(2).unwrap(),
            cancellations: CancellationRegistry::with_capacity(2).unwrap(),
        };
        for index in 0..10 {
            let query_id = format!("query-{index}");
            lifecycle.cancellations.token(&query_id).unwrap();
            let TaskClaim::Owner(owner) = lifecycle.tasks.claim(task(&query_id, 0)).unwrap() else {
                panic!("new sequential task must own its registry entry");
            };
            owner
                .complete(TaskOutcome::Success(Arc::new(Vec::new())))
                .unwrap();
            assert_eq!(lifecycle.finish_query(&query_id).unwrap(), 1);
            assert!(lifecycle.tasks.is_empty().unwrap());
        }
    }

    #[tokio::test]
    async fn cancellation_is_idempotent_and_wakes_existing_and_late_waiters() {
        let registry = CancellationRegistry::with_capacity(1).unwrap();
        let token = registry.token("query-a").unwrap();
        let waiter = token.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        assert!(registry.cancel("query-a").unwrap());
        assert!(registry.cancel("query-a").unwrap());
        task.await.unwrap();
        assert!(token.is_cancelled());
        token.cancelled().await;
        assert_eq!(
            registry.token("query-b").err(),
            Some(LifecycleError::QueryCapacityExceeded)
        );
        assert!(registry.remove("query-a").unwrap());
        assert!(!registry.cancel("query-a").unwrap());
        assert!(!registry.token("query-b").unwrap().is_cancelled());
    }
}
