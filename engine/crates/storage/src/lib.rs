#![deny(clippy::all)]

pub mod parquet_reader;

pub use parquet_reader::{ParquetBatchIterator, ParquetFileMetadata, ParquetReader};
