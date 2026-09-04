use crate::cluster::NodeInfo;

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

#[cfg(test)]
mod tests {
    use super::{RetryPolicy, task_candidates};
    use crate::cluster::{NodeInfo, NodeRole};

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
}
