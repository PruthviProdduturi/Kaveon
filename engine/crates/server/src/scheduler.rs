use crate::cluster::NodeInfo;
use kaveon_core::{ExchangeId, SplitDescriptor, StageId, TaskAssignment, TaskId};
use std::collections::{BTreeMap, VecDeque};

const DEFAULT_TASK_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_TASK_ATTEMPTS,
        }
    }
}

impl RetryPolicy {
    pub fn max_attempts(self) -> u32 {
        self.max_attempts
    }
}

pub fn task_candidates(
    workers: &[NodeInfo],
    partition: usize,
    policy: RetryPolicy,
) -> Vec<(u32, NodeInfo)> {
    if workers.is_empty() {
        return Vec::new();
    }

    let attempt_count = usize::try_from(policy.max_attempts())
        .unwrap_or(usize::MAX)
        .min(workers.len());
    (0..attempt_count)
        .map(|attempt| {
            let worker_index = (partition + attempt) % workers.len();
            (attempt as u32, workers[worker_index].clone())
        })
        .collect()
}

pub struct SplitScheduler {
    query_id: String,
    stage_id: StageId,
    pending: VecDeque<SplitDescriptor>,
    leased: BTreeMap<usize, Vec<SplitDescriptor>>,
    retries: VecDeque<(usize, u32)>,
    next_partition: usize,
    input_exchanges: Vec<ExchangeId>,
    output_exchanges: Vec<ExchangeId>,
}

impl SplitScheduler {
    pub fn new(
        query_id: impl Into<String>,
        stage_id: StageId,
        splits: Vec<SplitDescriptor>,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            stage_id,
            pending: splits.into(),
            leased: BTreeMap::new(),
            retries: VecDeque::new(),
            next_partition: 0,
            input_exchanges: Vec::new(),
            output_exchanges: Vec::new(),
        }
    }

    pub fn with_exchanges(
        mut self,
        input_exchanges: Vec<ExchangeId>,
        output_exchanges: Vec<ExchangeId>,
    ) -> Self {
        self.input_exchanges = input_exchanges;
        self.output_exchanges = output_exchanges;
        self
    }

    pub fn lease(
        &mut self,
        worker_id: impl Into<String>,
        split_limit: usize,
    ) -> Option<TaskAssignment> {
        if split_limit == 0 || self.pending.is_empty() {
            return None;
        }
        let (partition, attempt) = self.retries.pop_front().unwrap_or_else(|| {
            let partition = self.next_partition;
            self.next_partition = self.next_partition.saturating_add(1);
            (partition, 0)
        });
        let split_count = split_limit.min(self.pending.len());
        let splits = self.pending.drain(..split_count).collect::<Vec<_>>();
        self.leased.insert(partition, splits.clone());
        Some(TaskAssignment {
            task_id: TaskId {
                query_id: self.query_id.clone(),
                stage_id: self.stage_id,
                partition,
                attempt,
            },
            worker_id: worker_id.into(),
            splits,
            input_exchanges: self.input_exchanges.clone(),
            output_exchanges: self.output_exchanges.clone(),
        })
    }

    pub fn complete(&mut self, task_id: &TaskId) -> bool {
        self.leased.remove(&task_id.partition).is_some()
    }

    pub fn fail(&mut self, task_id: &TaskId) -> bool {
        let Some(splits) = self.leased.remove(&task_id.partition) else {
            return false;
        };
        for split in splits.into_iter().rev() {
            self.pending.push_front(split);
        }
        self.retries
            .push_back((task_id.partition, task_id.attempt.saturating_add(1)));
        true
    }

    pub fn pending_split_count(&self) -> usize {
        self.pending.len()
    }

    pub fn leased_task_count(&self) -> usize {
        self.leased.len()
    }

    pub fn is_finished(&self) -> bool {
        self.pending.is_empty() && self.leased.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{RetryPolicy, task_candidates};
    use crate::cluster::{NodeInfo, NodeRole};
    use kaveon_core::{SplitDescriptor, SplitId, StageId};
    use std::collections::BTreeMap;

    fn worker(node_id: &str) -> NodeInfo {
        NodeInfo {
            node_id: node_id.into(),
            role: NodeRole::Worker,
            address: format!("http://{node_id}:8080"),
            version: "test".into(),
            environment: "test".into(),
            uptime_secs: 0,
            last_heartbeat: 0,
            memory_rss_bytes: 0,
        }
    }

    #[test]
    fn rotates_primary_assignment_across_workers() {
        let workers = vec![worker("a"), worker("b"), worker("c")];

        let candidates = task_candidates(&workers, 1, RetryPolicy::default());

        assert_eq!(
            candidates
                .iter()
                .map(|(attempt, worker)| (*attempt, worker.node_id.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "b"), (1, "c"), (2, "a")]
        );
    }

    #[test]
    fn does_not_retry_a_worker_for_the_same_partition() {
        let workers = vec![worker("a"), worker("b")];

        let candidates = task_candidates(&workers, 8, RetryPolicy::default());

        assert_eq!(candidates.len(), workers.len());
        assert_ne!(candidates[0].1.node_id, candidates[1].1.node_id);
    }

    #[test]
    fn empty_cluster_has_no_candidates() {
        assert!(task_candidates(&[], 0, RetryPolicy::default()).is_empty());
    }

    fn split(id: &str) -> SplitDescriptor {
        SplitDescriptor {
            id: SplitId(id.into()),
            source_uri: format!("file:///{id}.parquet"),
            properties: BTreeMap::new(),
        }
    }

    #[test]
    fn leases_splits_independently_of_worker_count() {
        let mut scheduler = super::SplitScheduler::new(
            "query",
            StageId(2),
            vec![split("a"), split("b"), split("c")],
        );

        let first = scheduler.lease("worker-a", 2).unwrap();
        let second = scheduler.lease("worker-b", 2).unwrap();

        assert_eq!(first.splits.len(), 2);
        assert_eq!(second.splits.len(), 1);
        assert_eq!(scheduler.pending_split_count(), 0);
        assert_eq!(scheduler.leased_task_count(), 2);
        assert!(scheduler.complete(&first.task_id));
        assert!(scheduler.complete(&second.task_id));
        assert!(scheduler.is_finished());
    }

    #[test]
    fn failed_leases_return_splits_to_the_queue() {
        let mut scheduler =
            super::SplitScheduler::new("query", StageId(3), vec![split("a"), split("b")]);
        let failed = scheduler.lease("worker-a", 1).unwrap();

        assert!(scheduler.fail(&failed.task_id));
        assert_eq!(scheduler.pending_split_count(), 2);
        let retry = scheduler.lease("worker-b", 1).unwrap();
        assert_eq!(retry.splits[0].id, SplitId("a".into()));
        assert_eq!(retry.task_id.partition, failed.task_id.partition);
        assert_eq!(retry.task_id.attempt, failed.task_id.attempt + 1);
    }
}
