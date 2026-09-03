use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StageId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId {
    pub query_id: String,
    pub stage_id: StageId,
    pub partition: usize,
    pub attempt: u32,
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}.{}.{}.{}",
            self.query_id, self.stage_id.0, self.partition, self.attempt
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Partitioning {
    Single,
    Hash {
        columns: Vec<String>,
        partition_count: usize,
    },
    Broadcast,
    RoundRobin {
        partition_count: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::{StageId, TaskId};

    #[test]
    fn task_identity_includes_retry_attempt() {
        let first = TaskId {
            query_id: "query".into(),
            stage_id: StageId(2),
            partition: 7,
            attempt: 0,
        };
        let retry = TaskId {
            attempt: 1,
            ..first.clone()
        };
        assert_ne!(first, retry);
        assert_eq!(retry.to_string(), "query.2.7.1");
    }
}
