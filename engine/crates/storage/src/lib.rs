#![deny(clippy::all)]

pub mod delta_reader;
pub mod metrics;
pub mod parquet_reader;

pub use delta_reader::{DeltaBatchIterator, DeltaTableReader};
pub use metrics::{ScanMetrics, ScanMetricsSnapshot};
pub use parquet_reader::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader};
