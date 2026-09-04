use crate::cluster::NodeInfo;
use kaveon_core::{
    DataFormat, ExchangeId, KaveonError, Result, SplitDescriptor, SplitId, StageId, TaskAssignment,
    TaskId,
};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

const DEFAULT_TASK_ATTEMPTS: u32 = 3;
const FORMAT_PROPERTY: &str = "format";
const FILE_INDEX_PROPERTY: &str = "file_index";
const ROW_GROUP_INDEX_PROPERTY: &str = "row_group_index";

pub fn enumerate_local_splits(
    source: impl AsRef<Path>,
    format: DataFormat,
) -> Result<Vec<SplitDescriptor>> {
    let source = source.as_ref();
    match format {
        DataFormat::Parquet => enumerate_parquet_splits(source),
        DataFormat::Delta => enumerate_delta_splits(source),
        DataFormat::Iceberg => Err(KaveonError::Storage(
            "local Iceberg split enumeration is not implemented".into(),
        )),
    }
}

fn enumerate_parquet_splits(path: &Path) -> Result<Vec<SplitDescriptor>> {
    let row_group_count = ParquetReader::new(path).metadata()?.row_group_count;
    Ok((0..row_group_count)
        .map(|row_group| SplitDescriptor {
            id: SplitId(format!("parquet-row-group-{row_group}")),
            source_uri: local_file_uri(path),
            properties: BTreeMap::from([
                (FORMAT_PROPERTY.into(), "parquet".into()),
                (ROW_GROUP_INDEX_PROPERTY.into(), row_group.to_string()),
            ]),
        })
        .collect())
}

fn enumerate_delta_splits(path: &Path) -> Result<Vec<SplitDescriptor>> {
    DeltaTableReader::new(path)
        .active_file_paths()?
        .into_iter()
        .enumerate()
        .map(|(file_index, file)| {
            Ok(SplitDescriptor {
                id: SplitId(format!("delta-file-{file_index}")),
                source_uri: local_file_uri(&file),
                properties: BTreeMap::from([
                    (FORMAT_PROPERTY.into(), "parquet".into()),
                    (FILE_INDEX_PROPERTY.into(), file_index.to_string()),
                ]),
            })
        })
        .collect()
}

fn local_file_uri(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    format!("file:///{}", normalized.trim_start_matches('/'))
}

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
    leased: BTreeMap<usize, LeasedTask>,
    retries: VecDeque<(usize, u32)>,
    next_partition: usize,
    input_exchanges: Vec<ExchangeId>,
    output_exchanges: Vec<ExchangeId>,
}

struct LeasedTask {
    task_id: TaskId,
    splits: Vec<SplitDescriptor>,
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
        let task_id = TaskId {
            query_id: self.query_id.clone(),
            stage_id: self.stage_id,
            partition,
            attempt,
        };
        self.leased.insert(
            partition,
            LeasedTask {
                task_id: task_id.clone(),
                splits: splits.clone(),
            },
        );
        Some(TaskAssignment {
            task_id,
            worker_id: worker_id.into(),
            splits,
            input_exchanges: self.input_exchanges.clone(),
            output_exchanges: self.output_exchanges.clone(),
        })
    }

    pub fn complete(&mut self, task_id: &TaskId) -> bool {
        if !self
            .leased
            .get(&task_id.partition)
            .is_some_and(|lease| lease.task_id == *task_id)
        {
            return false;
        }
        self.leased.remove(&task_id.partition);
        true
    }

    pub fn fail(&mut self, task_id: &TaskId) -> bool {
        self.requeue(task_id)
    }

    pub fn requeue(&mut self, task_id: &TaskId) -> bool {
        if !self
            .leased
            .get(&task_id.partition)
            .is_some_and(|lease| lease.task_id == *task_id)
        {
            return false;
        }
        let lease = self
            .leased
            .remove(&task_id.partition)
            .expect("validated lease must remain present");
        for split in lease.splits.into_iter().rev() {
            self.pending.push_front(split);
        }
        self.retries
            .push_back((task_id.partition, task_id.attempt.saturating_add(1)));
        true
    }

    pub fn steal(
        &mut self,
        task_id: &TaskId,
        worker_id: impl Into<String>,
    ) -> Option<TaskAssignment> {
        let lease = self.leased.get_mut(&task_id.partition)?;
        if lease.task_id != *task_id {
            return None;
        }
        let next_task_id = TaskId {
            query_id: task_id.query_id.clone(),
            stage_id: task_id.stage_id,
            partition: task_id.partition,
            attempt: task_id.attempt.saturating_add(1),
        };
        lease.task_id = next_task_id.clone();
        Some(TaskAssignment {
            task_id: next_task_id,
            worker_id: worker_id.into(),
            splits: lease.splits.clone(),
            input_exchanges: self.input_exchanges.clone(),
            output_exchanges: self.output_exchanges.clone(),
        })
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
    use super::{RetryPolicy, enumerate_local_splits, task_candidates};
    use crate::cluster::{NodeInfo, NodeRole};
    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use kaveon_core::{DataFormat, SplitDescriptor, SplitId, StageId};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("kaveon-split-scheduler-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_parquet(path: &Path, row_group_size: usize) {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5]))],
        )
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let mut writer =
            ArrowWriter::try_new(File::create(path).unwrap(), schema, Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

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

        assert!(scheduler.requeue(&failed.task_id));
        assert_eq!(scheduler.pending_split_count(), 2);
        let retry = scheduler.lease("worker-b", 1).unwrap();
        assert_eq!(retry.splits[0].id, SplitId("a".into()));
        assert_eq!(retry.task_id.partition, failed.task_id.partition);
        assert_eq!(retry.task_id.attempt, failed.task_id.attempt + 1);
    }

    #[test]
    fn enumerates_one_deterministic_split_per_parquet_row_group() {
        let directory = TestDirectory::new();
        let path = directory.0.join("events.parquet");
        write_parquet(&path, 2);

        let first = enumerate_local_splits(&path, DataFormat::Parquet).unwrap();
        let second = enumerate_local_splits(&path, DataFormat::Parquet).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert_eq!(first[2].id, SplitId("parquet-row-group-2".into()));
        assert_eq!(first[2].properties["row_group_index"], "2");
        assert!(first[0].source_uri.starts_with("file:///"));
    }

    #[test]
    fn enumerates_delta_active_files_in_stable_snapshot_order() {
        let directory = TestDirectory::new();
        let log = directory.0.join("_delta_log");
        fs::create_dir_all(&log).unwrap();
        fs::write(
            log.join("00000000000000000000.json"),
            concat!(
                "{\"add\":{\"path\":\"z.parquet\"}}\n",
                "{\"add\":{\"path\":\"a.parquet\"}}\n"
            ),
        )
        .unwrap();
        fs::write(
            log.join("00000000000000000001.json"),
            concat!(
                "{\"remove\":{\"path\":\"z.parquet\"}}\n",
                "{\"add\":{\"path\":\"m.parquet\"}}\n"
            ),
        )
        .unwrap();

        let splits = enumerate_local_splits(&directory.0, DataFormat::Delta).unwrap();

        assert_eq!(splits.len(), 2);
        assert!(splits[0].source_uri.ends_with("/a.parquet"));
        assert!(splits[1].source_uri.ends_with("/m.parquet"));
        assert_eq!(splits[0].properties["file_index"], "0");
        assert_eq!(splits[1].properties["file_index"], "1");
    }

    #[test]
    fn stealing_a_skewed_tail_invalidates_the_original_attempt() {
        let splits = (0..17)
            .map(|index| split(&format!("split-{index}")))
            .collect();
        let mut scheduler = super::SplitScheduler::new("query", StageId(4), splits);
        let mut leases = Vec::new();
        while let Some(lease) = scheduler.lease("fast-worker", 4) {
            leases.push(lease);
        }
        for lease in leases.iter().take(leases.len() - 1) {
            assert!(scheduler.complete(&lease.task_id));
        }
        let tail = leases.last().unwrap();

        let stolen = scheduler.steal(&tail.task_id, "idle-worker").unwrap();

        assert_eq!(stolen.splits, tail.splits);
        assert_eq!(stolen.task_id.attempt, tail.task_id.attempt + 1);
        assert!(!scheduler.complete(&tail.task_id));
        assert!(!scheduler.fail(&tail.task_id));
        assert!(scheduler.complete(&stolen.task_id));
        assert!(scheduler.is_finished());
    }
}
