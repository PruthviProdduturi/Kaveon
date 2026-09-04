use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{KaveonError, PlanNode, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StageId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId {
    pub query_id: String,
    pub stage_id: StageId,
    pub partition: usize,
    pub attempt: u32,
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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

impl Partitioning {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Hash {
                columns,
                partition_count,
            } => {
                if columns.is_empty() {
                    return Err(KaveonError::Execution(
                        "hash partitioning requires at least one column".into(),
                    ));
                }
                validate_partition_count(*partition_count)
            }
            Self::RoundRobin { partition_count } => validate_partition_count(*partition_count),
            Self::Single | Self::Broadcast => Ok(()),
        }
    }
}

fn validate_partition_count(partition_count: usize) -> Result<()> {
    if partition_count == 0 {
        return Err(KaveonError::Execution(
            "partition count must be greater than zero".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExchangeId(pub String);

impl fmt::Display for ExchangeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeDescriptor {
    pub id: ExchangeId,
    pub source_stage: StageId,
    pub target_stage: StageId,
    pub partitioning: Partitioning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFragment {
    pub id: StageId,
    pub task_count: usize,
    pub plan: PlanNode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageGraph {
    pub query_id: String,
    pub root_stage: StageId,
    pub stages: Vec<StageFragment>,
    pub exchanges: Vec<ExchangeDescriptor>,
}

impl StageGraph {
    pub fn validate(&self) -> Result<()> {
        if self.query_id.trim().is_empty() {
            return Err(KaveonError::Execution(
                "stage graph query ID cannot be empty".into(),
            ));
        }
        let mut stage_ids = BTreeSet::new();
        for stage in &self.stages {
            if stage.task_count == 0 {
                return Err(KaveonError::Execution(format!(
                    "stage {} must have at least one task",
                    stage.id.0
                )));
            }
            if !stage_ids.insert(stage.id) {
                return Err(KaveonError::Execution(format!(
                    "duplicate stage ID {}",
                    stage.id.0
                )));
            }
        }
        if !stage_ids.contains(&self.root_stage) {
            return Err(KaveonError::Execution(format!(
                "root stage {} does not exist",
                self.root_stage.0
            )));
        }

        let mut exchange_ids = BTreeSet::new();
        let mut edges = BTreeMap::<StageId, Vec<StageId>>::new();
        for exchange in &self.exchanges {
            if exchange.id.0.trim().is_empty() {
                return Err(KaveonError::Execution("exchange ID cannot be empty".into()));
            }
            if !exchange_ids.insert(exchange.id.clone()) {
                return Err(KaveonError::Execution(format!(
                    "duplicate exchange ID {}",
                    exchange.id
                )));
            }
            if exchange.source_stage == exchange.target_stage {
                return Err(KaveonError::Execution(format!(
                    "exchange {} cannot connect a stage to itself",
                    exchange.id
                )));
            }
            if !stage_ids.contains(&exchange.source_stage)
                || !stage_ids.contains(&exchange.target_stage)
            {
                return Err(KaveonError::Execution(format!(
                    "exchange {} references an unknown stage",
                    exchange.id
                )));
            }
            exchange.partitioning.validate()?;
            edges
                .entry(exchange.source_stage)
                .or_default()
                .push(exchange.target_stage);
        }
        ensure_acyclic(&stage_ids, &edges)
    }
}

fn ensure_acyclic(
    stage_ids: &BTreeSet<StageId>,
    edges: &BTreeMap<StageId, Vec<StageId>>,
) -> Result<()> {
    fn visit(
        stage: StageId,
        edges: &BTreeMap<StageId, Vec<StageId>>,
        visiting: &mut BTreeSet<StageId>,
        visited: &mut BTreeSet<StageId>,
    ) -> Result<()> {
        if visited.contains(&stage) {
            return Ok(());
        }
        if !visiting.insert(stage) {
            return Err(KaveonError::Execution(format!(
                "stage graph contains a cycle at stage {}",
                stage.0
            )));
        }
        if let Some(targets) = edges.get(&stage) {
            for target in targets {
                visit(*target, edges, visiting, visited)?;
            }
        }
        visiting.remove(&stage);
        visited.insert(stage);
        Ok(())
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for stage in stage_ids {
        visit(*stage, edges, &mut visiting, &mut visited)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SplitId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitDescriptor {
    pub id: SplitId,
    pub source_uri: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task_id: TaskId,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splits: Vec<SplitDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_exchanges: Vec<ExchangeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_exchanges: Vec<ExchangeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Finished,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatus {
    pub task_id: TaskId,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlanPhase;

    fn fragment(id: u32) -> StageFragment {
        StageFragment {
            id: StageId(id),
            task_count: 1,
            plan: PlanNode::new(id, PlanPhase::Physical, "test"),
        }
    }

    fn graph(exchanges: Vec<ExchangeDescriptor>) -> StageGraph {
        StageGraph {
            query_id: "query".into(),
            root_stage: StageId(2),
            stages: vec![fragment(1), fragment(2)],
            exchanges,
        }
    }

    fn exchange(source: u32, target: u32) -> ExchangeDescriptor {
        ExchangeDescriptor {
            id: ExchangeId(format!("exchange-{source}-{target}")),
            source_stage: StageId(source),
            target_stage: StageId(target),
            partitioning: Partitioning::Hash {
                columns: vec!["customer_id".into()],
                partition_count: 4,
            },
        }
    }

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

    #[test]
    fn accepts_a_valid_multi_stage_graph() {
        graph(vec![exchange(1, 2)]).validate().unwrap();
    }

    #[test]
    fn rejects_cycles_in_the_stage_graph() {
        let error = graph(vec![exchange(1, 2), exchange(2, 1)])
            .validate()
            .unwrap_err();
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn rejects_unknown_exchange_stages() {
        let error = graph(vec![exchange(3, 2)]).validate().unwrap_err();
        assert!(error.to_string().contains("unknown stage"));
    }

    #[test]
    fn rejects_empty_hash_keys_and_partition_sets() {
        let no_keys = Partitioning::Hash {
            columns: Vec::new(),
            partition_count: 2,
        };
        let no_partitions = Partitioning::RoundRobin { partition_count: 0 };
        assert!(no_keys.validate().is_err());
        assert!(no_partitions.validate().is_err());
    }

    #[test]
    fn task_assignments_preserve_attempt_and_split_identity() {
        let assignment = TaskAssignment {
            task_id: TaskId {
                query_id: "query".into(),
                stage_id: StageId(1),
                partition: 0,
                attempt: 2,
            },
            worker_id: "worker-a".into(),
            splits: vec![SplitDescriptor {
                id: SplitId("split-7".into()),
                source_uri: "file:///events.parquet".into(),
                properties: BTreeMap::new(),
            }],
            input_exchanges: Vec::new(),
            output_exchanges: vec![ExchangeId("exchange-1-2".into())],
        };
        assert_eq!(assignment.task_id.attempt, 2);
        assert_eq!(assignment.splits[0].id, SplitId("split-7".into()));
    }
}
