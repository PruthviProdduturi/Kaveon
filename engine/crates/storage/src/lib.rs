#![deny(clippy::all)]

pub mod adls_commit;
pub mod adls_reader;
pub mod clustered_writer;
pub mod cube;
pub mod delta_reader;
pub mod delta_snapshot;
pub mod footer_profile;
pub mod iceberg_reader;
pub mod immutable_data_writer;
pub mod metrics;
pub mod object_delta;
pub mod object_reader;
pub mod parquet_directory;
pub mod parquet_reader;
#[cfg(test)]
mod scan_bench;
pub mod scan_predicate;
pub mod source_statistics;
pub mod table_rewrite;
pub mod table_statistics;

pub use adls_commit::{
    AdlsConditionalCommit, CommitError, CommitErrorKind, CommitResult, ObjectVersion,
    VersionedObject, local_file_commit, workload_identity_adls_commit,
};
pub use adls_reader::{AdlsAuthMode, AdlsBatchSource, AdlsBatchStream, AdlsParquetReader};
pub use clustered_writer::{
    ClusteredFileSink, ClusteredParquetWriter, ClusteringLayout, FileNaming, LocalDirectorySink,
    MemorySink, WrittenFile,
};
pub use cube::{CubeBuild, CubeBuildOptions, build_cube, refresh_cube};
pub use delta_reader::{DeltaBatchIterator, DeltaTableReader};
pub use footer_profile::{ColumnProfile, FooterProfile, StatValue};
pub use iceberg_reader::{IcebergReader, IcebergSnapshot, IcebergSource};
pub use immutable_data_writer::{
    DEFAULT_MAX_PARQUET_BYTES, DataWriteError, ImmutableDataReference, ImmutableParquetWriter,
};
pub use metrics::{ScanMetrics, ScanMetricsSnapshot};
pub use object_delta::{ObjectDeltaReader, ObjectDeltaSource};
pub use object_reader::{ObjectBatchSource, ObjectLocation, ObjectParquetReader};
pub use parquet_directory::{
    AssignedFile, AssignedFiles, DirectoryFile, DirectoryListing, FileAssignment, FileSlice,
    HIVE_DEFAULT_PARTITION, KeptFile, ObjectDirectoryReader, ObjectDirectorySource,
    ParquetLocation, PartitionLayout, PartitionValue, PrunedFiles, assign_files,
    assign_files_to_partitions, assign_kept_files, directory_row_count, directory_row_group_counts,
    list_parquet_directory, parquet_listing_at, partition_field, prune_files, verify_listing,
};
pub use parquet_reader::{
    ParquetBatchIterator, ParquetFileMetadata, ParquetReader, footer_may_match,
};
pub use scan_predicate::LateMaterialisation;
pub use source_statistics::{
    SourceColumnProfile, SourceProfile, SourceStatistics, analyze_source, current_source_version,
    profile_source, profile_source_version,
};
pub use table_rewrite::{
    Recovery, RewriteGroup, RewriteKind, RewriteReport, RewriteTarget, Staging, TableFile,
};
pub use table_statistics::{
    DataFile, FullScanOptions, SourceFiles, enumerate_source, full_statistics, metadata_statistics,
    partition_column_names, refresh_statistics, sketchable, skip_listing_files, statistics_schema,
};

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
