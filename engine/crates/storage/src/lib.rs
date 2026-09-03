#![deny(clippy::all)]

pub mod delta_reader;
pub mod parquet_reader;

pub use delta_reader::{DeltaBatchIterator, DeltaTableReader};
pub use parquet_reader::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader};
