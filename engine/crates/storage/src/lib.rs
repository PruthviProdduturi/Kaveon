#![deny(clippy::all)]

pub mod adls_reader;
pub mod delta_reader;
pub mod metrics;
pub mod parquet_reader;

pub use adls_reader::{AdlsAuthMode, AdlsBatchSource, AdlsBatchStream, AdlsParquetReader};
pub use delta_reader::{DeltaBatchIterator, DeltaTableReader};
pub use metrics::{ScanMetrics, ScanMetricsSnapshot};
pub use parquet_reader::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader};

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
