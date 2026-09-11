#![deny(clippy::all)]

pub mod adls_commit;
pub mod adls_reader;
pub mod delta_reader;
pub mod delta_snapshot;
pub mod iceberg_reader;
pub mod immutable_data_writer;
pub mod metrics;
pub mod object_delta;
pub mod object_reader;
pub mod parquet_reader;
pub mod source_statistics;

pub use adls_commit::{
    AdlsConditionalCommit, CommitError, CommitErrorKind, CommitResult, ObjectVersion,
    VersionedObject, workload_identity_adls_commit,
};
pub use adls_reader::{AdlsAuthMode, AdlsBatchSource, AdlsBatchStream, AdlsParquetReader};
pub use delta_reader::{DeltaBatchIterator, DeltaTableReader};
pub use iceberg_reader::{IcebergReader, IcebergSnapshot, IcebergSource};
pub use immutable_data_writer::{
    DataWriteError, ImmutableDataReference, ImmutableParquetWriter,
    DEFAULT_MAX_PARQUET_BYTES,
};
pub use metrics::{ScanMetrics, ScanMetricsSnapshot};
pub use object_delta::{ObjectDeltaReader, ObjectDeltaSource};
pub use object_reader::{ObjectBatchSource, ObjectLocation, ObjectParquetReader};
pub use parquet_reader::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader};
pub use source_statistics::{SourceStatistics, analyze_source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanPartition {
    pub index: usize,
    pub count: usize,
}

impl ScanPartition {
    pub fn new(index: usize, count: usize) -> kaveon_core::Result<Self> {
        if count == 0 {
            return Err(kaveon_core::KaveonError::Storage(
                "scan partition count must be greater than zero".into(),
            ));
        }
        if index >= count {
            return Err(kaveon_core::KaveonError::Storage(format!(
                "scan partition index {index} is outside partition count {count}"
            )));
        }
        Ok(Self { index, count })
    }

    pub fn contains(self, ordinal: usize) -> bool {
        ordinal % self.count == self.index
    }
}

#[cfg(test)]
mod tests {
    use super::ScanPartition;

    #[test]
    fn partitions_cover_ordinals_once_without_overlap() {
        let partitions = (0..3)
            .map(|index| ScanPartition::new(index, 3).unwrap())
            .collect::<Vec<_>>();
        for ordinal in 0..100 {
            assert_eq!(
                partitions
                    .iter()
                    .filter(|partition| partition.contains(ordinal))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn rejects_invalid_partitions() {
        assert!(ScanPartition::new(0, 0).is_err());
        assert!(ScanPartition::new(2, 2).is_err());
    }
}
