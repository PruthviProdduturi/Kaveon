//! Clustered Parquet layout: files whose row groups the readers can skip.
//!
//! The Engine reads better than it has laid data out: a table written as
//! one file of ~1 M-row row groups without a page index still touches every
//! row group for a filter on a column that looks clustered. This module is
//! the write side that makes skipping bite. [`ClusteredParquetWriter`] takes
//! batches **in clustering order** and emits files in the layout the
//! readers prune by:
//!
//! - row groups of a target size — [`DEFAULT_TARGET_ROW_GROUP_BYTES`] of
//!   encoded, compressed bytes or [`DEFAULT_MAX_ROW_GROUP_ROWS`] rows,
//!   whichever comes first — so a row group's min/max statistics span a
//!   narrow key range and the row-group pruning in `parquet_reader` and
//!   `adls_reader` drops the groups a point or range filter cannot touch;
//! - the page index (column index and offset index) so a row filter skips
//!   the pages a selection never reaches, at [`DEFAULT_PAGE_ROWS`] rows a
//!   page (parquet-mr's default; the offset index is what late
//!   materialisation reads to fetch only the pages it needs);
//! - a Bloom filter per row group on every clustering column and every
//!   column declared `bloom = ARRAY[…]`, at [`BLOOM_FILTER_FPP`], sized for
//!   the row group's row cap: a point lookup on a sparse key can reject a
//!   row group whose min/max admit it;
//! - dictionary encoding (the readers' dictionary-aware predicates and the
//!   columnar aggregate's arena keys run over dictionaries), page-level
//!   statistics, ZSTD compression, and the clustering order recorded in the
//!   footer's `sorting_columns` and `kaveon.layout` key-value metadata.
//!
//! The writer holds one in-progress row group at a time (parquet-rs
//! buffers a row group's encoded pages until it is flushed), so its memory
//! is bounded by the row-group target, never by the file. The **sort** is
//! not in this module: the storage crate sits below the executor, and the
//! executor's `SortOperator` is already the bounded external sort over the
//! query memory pool and the spill machinery. `OPTIMIZE` composes the two
//! (`kaveon_server::optimize`); the writer verifies the order it is given
//! and refuses an out-of-order batch by row, so a wrong composition fails
//! closed instead of writing a file the footer calls sorted.
//!
//! Files are written through a [`ClusteredFileSink`]: a local directory
//! ([`LocalDirectorySink`]) for a local table or as a staging area before an
//! upload, an in-memory sink in tests.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arrow::array::ArrayRef;
use arrow::compute::SortOptions;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use arrow::row::{OwnedRow, RowConverter, SortField};
use kaveon_core::{BatchOperator, KaveonError, Result};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::format::SortingColumn;
use parquet::schema::types::ColumnPath;

/// Encoded, compressed bytes a row group is closed at: 128 MiB, the row
/// group size Spark and Trino write and the unit the readers prune by.
pub const DEFAULT_TARGET_ROW_GROUP_BYTES: u64 = 128 * 1024 * 1024;
/// Rows a row group is closed at when the byte target is not reached
/// first: a narrow table would otherwise put tens of millions of rows in
/// one group and a filter would examine them all.
pub const DEFAULT_MAX_ROW_GROUP_ROWS: usize = 1 << 20;
/// A file is closed at the first row-group boundary past this many bytes.
pub const DEFAULT_TARGET_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// Rows per data page: the unit the offset index lets a reader skip.
pub const DEFAULT_PAGE_ROWS: usize = 20_000;
/// Bytes per data page, the other page limit.
pub const DEFAULT_PAGE_BYTES: usize = 1024 * 1024;
/// False-positive probability of the Bloom filters.
pub const BLOOM_FILTER_FPP: f64 = 0.01;
/// The footer key-value entry that records the clustering columns.
pub const LAYOUT_METADATA_KEY: &str = "kaveon.layout.clustered_by";

/// The layout a table's files are written in.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusteringLayout {
    /// Columns rows are sorted by within every file, ascending, NULLs last.
    pub clustered_by: Vec<String>,
    /// Columns that carry a Bloom filter per row group; the clustering
    /// columns are added whether or not they are listed.
    pub bloom: Vec<String>,
    pub target_row_group_bytes: u64,
    pub max_row_group_rows: usize,
    /// `None` writes one file whatever its size.
    pub target_file_bytes: Option<u64>,
    pub page_rows: usize,
    pub page_bytes: usize,
}

impl ClusteringLayout {
    /// The default sizes, clustered by `clustered_by` with Bloom filters on
    /// `bloom`.
    pub fn new(clustered_by: Vec<String>, bloom: Vec<String>) -> Self {
        Self {
            clustered_by,
            bloom,
            target_row_group_bytes: DEFAULT_TARGET_ROW_GROUP_BYTES,
            max_row_group_rows: DEFAULT_MAX_ROW_GROUP_ROWS,
            target_file_bytes: Some(DEFAULT_TARGET_FILE_BYTES),
            page_rows: DEFAULT_PAGE_ROWS,
            page_bytes: DEFAULT_PAGE_BYTES,
        }
    }

    pub fn with_max_row_group_rows(mut self, rows: usize) -> Self {
        self.max_row_group_rows = rows;
        self
    }

    pub fn with_target_row_group_bytes(mut self, bytes: u64) -> Self {
        self.target_row_group_bytes = bytes;
        self
    }

    pub fn with_target_file_bytes(mut self, bytes: Option<u64>) -> Self {
        self.target_file_bytes = bytes;
        self
    }

    /// Every column that carries a Bloom filter, clustering columns first.
    pub fn bloom_columns(&self) -> Vec<String> {
        let mut columns = self.clustered_by.clone();
        for column in &self.bloom {
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        }
        columns
    }

    fn validate(&self, schema: &SchemaRef) -> Result<()> {
        if self.max_row_group_rows == 0 {
            return Err(invalid("row group row limit must be greater than zero"));
        }
        if self.target_row_group_bytes == 0 {
            return Err(invalid("row group byte target must be greater than zero"));
        }
        if self.page_rows == 0 || self.page_bytes == 0 {
            return Err(invalid("page limits must be greater than zero"));
        }
        if self.target_file_bytes == Some(0) {
            return Err(invalid("file byte target must be greater than zero"));
        }
        let mut seen = std::collections::HashSet::new();
        for column in &self.clustered_by {
            if schema.index_of(column).is_err() {
                return Err(invalid(format!(
                    "clustering column '{column}' is not in the schema"
                )));
            }
            if !seen.insert(column.as_str()) {
                return Err(invalid(format!(
                    "clustering column '{column}' is listed twice"
                )));
            }
        }
        for column in &self.bloom {
            if schema.index_of(column).is_err() {
                return Err(invalid(format!(
                    "bloom column '{column}' is not in the schema"
                )));
            }
        }
        Ok(())
    }

    /// The parquet writer properties for this layout over `schema`.
    pub fn writer_properties(&self, schema: &SchemaRef) -> Result<WriterProperties> {
        let parquet_schema = parquet::arrow::ArrowSchemaConverter::new()
            .convert(schema)
            .map_err(|error| invalid(error.to_string()))?;
        let leaf_index = |column: &str| {
            parquet_schema
                .columns()
                .iter()
                .position(|leaf| leaf.path().parts().first().map(String::as_str) == Some(column))
                .and_then(|index| i32::try_from(index).ok())
                .ok_or_else(|| invalid(format!("column '{column}' has no Parquet leaf")))
        };
        let sorting = self
            .clustered_by
            .iter()
            .map(|column| {
                Ok(SortingColumn {
                    column_idx: leaf_index(column)?,
                    descending: false,
                    nulls_first: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let ndv = u64::try_from(self.max_row_group_rows).unwrap_or(u64::MAX);
        let mut builder = WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            .set_created_by(format!("kaveon-storage {}", env!("CARGO_PKG_VERSION")))
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(3).map_err(|error| invalid(error.to_string()))?,
            ))
            .set_dictionary_enabled(true)
            .set_statistics_enabled(EnabledStatistics::Page)
            .set_offset_index_disabled(false)
            .set_max_row_group_size(self.max_row_group_rows)
            .set_data_page_row_count_limit(self.page_rows)
            .set_data_page_size_limit(self.page_bytes)
            .set_bloom_filter_enabled(false)
            .set_key_value_metadata(Some(vec![KeyValue::new(
                LAYOUT_METADATA_KEY.to_owned(),
                serde_json::to_string(&self.clustered_by)
                    .map_err(|error| invalid(error.to_string()))?,
            )]));
        if !sorting.is_empty() {
            builder = builder.set_sorting_columns(Some(sorting));
        }
        for column in self.bloom_columns() {
            let path = ColumnPath::from(column.as_str());
            builder = builder
                .set_column_bloom_filter_enabled(path.clone(), true)
                .set_column_bloom_filter_fpp(path.clone(), BLOOM_FILTER_FPP)
                .set_column_bloom_filter_ndv(path, ndv);
        }
        Ok(builder.build())
    }
}

/// Where the writer's files go.
pub trait ClusteredFileSink: Send {
    /// Open the file called `name` for writing; the writer closes it.
    fn create(&mut self, name: &str) -> Result<Box<dyn Write + Send>>;
}

/// Files under one local directory, created as the writer opens them.
pub struct LocalDirectorySink {
    directory: PathBuf,
}

impl LocalDirectorySink {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory).map_err(|error| {
            KaveonError::Storage(format!("cannot create '{}': {error}", directory.display()))
        })?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }
}

impl ClusteredFileSink for LocalDirectorySink {
    fn create(&mut self, name: &str) -> Result<Box<dyn Write + Send>> {
        let path = self.directory.join(name);
        let file = std::fs::File::create(&path).map_err(|error| {
            KaveonError::Storage(format!("cannot create '{}': {error}", path.display()))
        })?;
        Ok(Box::new(std::io::BufWriter::with_capacity(
            1024 * 1024,
            file,
        )))
    }
}

/// Files held in memory by name.
#[derive(Clone, Default)]
pub struct MemorySink {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemorySink {
    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        self.files.lock().ok()?.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names = self
            .files
            .lock()
            .map(|files| files.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        names.sort();
        names
    }
}

struct MemoryFile {
    name: String,
    bytes: Vec<u8>,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl Write for MemoryFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Ok(mut files) = self.files.lock() {
            files.insert(self.name.clone(), self.bytes.clone());
        }
        Ok(())
    }
}

impl Drop for MemoryFile {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl ClusteredFileSink for MemorySink {
    fn create(&mut self, name: &str) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(MemoryFile {
            name: name.to_owned(),
            bytes: Vec::new(),
            files: Arc::clone(&self.files),
        }))
    }
}

/// How the writer names its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileNaming {
    /// Exactly one file with this name; the file byte target does not
    /// apply.
    Single(String),
    /// `<prefix>-<ordinal>.parquet`, a new file at every file byte target.
    Parts { prefix: String },
}

/// One file the writer finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenFile {
    pub name: String,
    pub rows: u64,
    pub row_groups: usize,
    pub bytes: u64,
}

struct OpenFile {
    name: String,
    writer: ArrowWriter<Box<dyn Write + Send>>,
    rows: u64,
}

/// Checks that the batches arrive in clustering order.
struct OrderCheck {
    converter: RowConverter,
    indices: Vec<usize>,
    last: Option<OwnedRow>,
    rows_seen: u64,
}

impl OrderCheck {
    fn new(schema: &SchemaRef, clustered_by: &[String]) -> Result<Self> {
        let indices = clustered_by
            .iter()
            .map(|column| {
                schema.index_of(column).map_err(|_| {
                    invalid(format!("clustering column '{column}' is not in the schema"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let fields = indices
            .iter()
            .map(|index| {
                SortField::new_with_options(
                    schema.field(*index).data_type().clone(),
                    SortOptions {
                        descending: false,
                        nulls_first: false,
                    },
                )
            })
            .collect();
        let converter = RowConverter::new(fields).map_err(|error| invalid(error.to_string()))?;
        Ok(Self {
            converter,
            indices,
            last: None,
            rows_seen: 0,
        })
    }

    fn check(&mut self, batch: &RecordBatch) -> Result<()> {
        let columns = self
            .indices
            .iter()
            .map(|index| Arc::clone(batch.column(*index)))
            .collect::<Vec<ArrayRef>>();
        let rows = self
            .converter
            .convert_columns(&columns)
            .map_err(|error| invalid(error.to_string()))?;
        let mut previous = self.last.take();
        for (offset, row) in rows.iter().enumerate() {
            if let Some(previous) = &previous
                && previous.row() > row
            {
                return Err(KaveonError::Storage(format!(
                    "rows are not in clustering order: row {} sorts before row {}",
                    self.rows_seen + offset as u64,
                    self.rows_seen + offset as u64 - 1
                )));
            }
            previous = Some(row.owned());
        }
        self.last = previous;
        self.rows_seen += batch.num_rows() as u64;
        Ok(())
    }
}

/// Writes batches given in clustering order as files in the clustered
/// layout. See the module documentation.
pub struct ClusteredParquetWriter {
    layout: ClusteringLayout,
    schema: SchemaRef,
    properties: WriterProperties,
    naming: FileNaming,
    sink: Box<dyn ClusteredFileSink>,
    current: Option<OpenFile>,
    files: Vec<WrittenFile>,
    order: Option<OrderCheck>,
}

impl ClusteredParquetWriter {
    pub fn new(
        layout: ClusteringLayout,
        schema: SchemaRef,
        naming: FileNaming,
        sink: Box<dyn ClusteredFileSink>,
    ) -> Result<Self> {
        layout.validate(&schema)?;
        if let FileNaming::Parts { prefix } = &naming
            && prefix.is_empty()
        {
            return Err(invalid("file name prefix cannot be empty"));
        }
        let properties = layout.writer_properties(&schema)?;
        let order = if layout.clustered_by.is_empty() {
            None
        } else {
            Some(OrderCheck::new(&schema, &layout.clustered_by)?)
        };
        Ok(Self {
            layout,
            schema,
            properties,
            naming,
            sink,
            current: None,
            files: Vec::new(),
            order,
        })
    }

    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Write one batch; it must follow the previous batches in clustering
    /// order and carry the writer's schema.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        if batch.schema().fields() != self.schema.fields() {
            return Err(invalid(format!(
                "batch schema {:?} does not match the table schema {:?}",
                batch.schema().fields(),
                self.schema.fields()
            )));
        }
        if let Some(order) = &mut self.order {
            order.check(batch)?;
        }
        // The batch goes in by row-group-sized slices, so a file closes
        // exactly at a row-group boundary whatever the batch size.
        let mut offset = 0;
        while offset < batch.num_rows() {
            let room =
                self.layout.max_row_group_rows - self.current_file()?.writer.in_progress_rows();
            let length = room.max(1).min(batch.num_rows() - offset);
            self.write_slice(&batch.slice(offset, length))?;
            offset += length;
        }
        Ok(())
    }

    fn current_file(&mut self) -> Result<&mut OpenFile> {
        if self.current.is_none() {
            let name = self.next_name();
            let writer = ArrowWriter::try_new(
                self.sink.create(&name)?,
                Arc::clone(&self.schema),
                Some(self.properties.clone()),
            )
            .map_err(parquet_error)?;
            self.current = Some(OpenFile {
                name,
                writer,
                rows: 0,
            });
        }
        Ok(self
            .current
            .as_mut()
            .expect("the current file was just opened"))
    }

    /// One slice that fits the in-progress row group's room.
    fn write_slice(&mut self, slice: &RecordBatch) -> Result<()> {
        let target_row_group_bytes = self.layout.target_row_group_bytes;
        let file_target = match (&self.naming, self.layout.target_file_bytes) {
            (FileNaming::Parts { .. }, Some(target)) => Some(target),
            _ => None,
        };
        let file = self.current_file()?;
        file.writer.write(slice).map_err(parquet_error)?;
        file.rows += slice.num_rows() as u64;
        // The row cap is the writer's own; the byte target closes the row
        // group here, at the slice boundary past it.
        if file.writer.in_progress_rows() > 0
            && file.writer.in_progress_size() as u64 >= target_row_group_bytes
        {
            file.writer.flush().map_err(parquet_error)?;
        }
        if let Some(target) = file_target
            && file.writer.in_progress_rows() == 0
            && file.writer.bytes_written() as u64 >= target
        {
            self.close_current()?;
        }
        Ok(())
    }

    /// Drain `source` into the writer.
    pub fn write_all(&mut self, source: &mut dyn BatchOperator) -> Result<()> {
        while let Some(batch) = source.next_batch()? {
            self.write(&batch)?;
        }
        Ok(())
    }

    /// Close the last file and report every file written, in order. No
    /// file is written for no rows.
    pub fn finish(mut self) -> Result<Vec<WrittenFile>> {
        self.close_current()?;
        Ok(std::mem::take(&mut self.files))
    }

    fn next_name(&self) -> String {
        match &self.naming {
            FileNaming::Single(name) => name.clone(),
            FileNaming::Parts { prefix } => {
                format!("{prefix}-{:05}.parquet", self.files.len())
            }
        }
    }

    fn close_current(&mut self) -> Result<()> {
        let Some(file) = self.current.take() else {
            return Ok(());
        };
        let OpenFile {
            name,
            mut writer,
            rows,
        } = file;
        // `finish` writes the footer and leaves the byte count readable;
        // the sink's file is flushed when the writer drops it.
        let metadata = writer.finish().map_err(parquet_error)?;
        let bytes = writer.bytes_written() as u64;
        writer
            .inner_mut()
            .flush()
            .map_err(|error| KaveonError::Storage(format!("cannot flush '{name}': {error}")))?;
        drop(writer);
        self.files.push(WrittenFile {
            name,
            rows,
            row_groups: metadata.row_groups.len(),
            bytes,
        });
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

fn parquet_error(error: parquet::errors::ParquetError) -> KaveonError {
    KaveonError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdlsParquetReader, LateMaterialisation, ParquetReader, ScanMetrics};
    use arrow::array::{Int64Array, StringArray};
    use arrow::compute::{sort_to_indices, take};
    use arrow::datatypes::{DataType, Field, Schema};
    use kaveon_core::{BatchSource, CompareOp, ScalarValue, StoragePredicate};
    use object_store::{ObjectStore, memory::InMemory, path::Path};
    use parquet::file::reader::{FileReader, SerializedFileReader};

    /// A deterministic mixer: the same rows on every run and machine.
    fn mix(seed: u64) -> u64 {
        let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        value ^= value >> 29;
        value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value ^ (value >> 32)
    }

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("payload", DataType::Utf8, false),
            Field::new("value", DataType::Int64, true),
        ]))
    }

    /// `rows` rows whose key is scattered over `0..rows`, in row order.
    fn scattered(rows: usize) -> RecordBatch {
        let keys = (0..rows as u64)
            .map(|row| (mix(row) % rows as u64) as i64)
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(Int64Array::from(keys.clone())),
                Arc::new(StringArray::from_iter_values(
                    keys.iter().map(|key| format!("payload-{key:0>32}")),
                )),
                Arc::new(Int64Array::from(
                    keys.iter()
                        .map(|key| (key % 7 != 0).then_some(key * 3))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap()
    }

    fn sorted_by_key(batch: &RecordBatch) -> RecordBatch {
        let indices = sort_to_indices(batch.column(0), None, None).unwrap();
        let columns = batch
            .columns()
            .iter()
            .map(|column| take(column, &indices, None).unwrap())
            .collect();
        RecordBatch::try_new(batch.schema(), columns).unwrap()
    }

    fn write_clustered(
        layout: ClusteringLayout,
        naming: FileNaming,
        batches: &[RecordBatch],
    ) -> (MemorySink, Vec<WrittenFile>) {
        let sink = MemorySink::default();
        let mut writer =
            ClusteredParquetWriter::new(layout, schema(), naming, Box::new(sink.clone())).unwrap();
        for batch in batches {
            writer.write(batch).unwrap();
        }
        let files = writer.finish().unwrap();
        (sink, files)
    }

    /// The rows written the way `hits.parquet` was: one file, unsorted,
    /// million-row row groups, chunk statistics only, no page index.
    fn write_plain(batch: &RecordBatch) -> Vec<u8> {
        let properties = WriterProperties::builder()
            .set_max_row_group_size(1_000_000)
            .set_statistics_enabled(EnabledStatistics::Chunk)
            .set_offset_index_disabled(true)
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
            .build();
        let mut bytes = Vec::new();
        let mut writer =
            ArrowWriter::try_new(&mut bytes, batch.schema(), Some(properties)).unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
        bytes
    }

    fn point(key: i64) -> StoragePredicate {
        StoragePredicate::Compare {
            column: "key".into(),
            op: CompareOp::Eq,
            value: ScalarValue::Int64(key),
        }
    }

    fn range(low: i64, high: i64) -> StoragePredicate {
        StoragePredicate::And(vec![
            StoragePredicate::Compare {
                column: "key".into(),
                op: CompareOp::Ge,
                value: ScalarValue::Int64(low),
            },
            StoragePredicate::Compare {
                column: "key".into(),
                op: CompareOp::Lt,
                value: ScalarValue::Int64(high),
            },
        ])
    }

    fn sum_values(batch: &RecordBatch) -> i64 {
        batch
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .iter()
            .flatten()
            .sum()
    }

    /// (rows, sum of value) through the local reader with `predicate`, and
    /// the scan metrics.
    fn read_local(
        path: &std::path::Path,
        predicate: &StoragePredicate,
    ) -> ((u64, i64), crate::ScanMetricsSnapshot) {
        let metrics = ScanMetrics::default();
        let reader = ParquetReader::new(path)
            .with_predicate(predicate.clone())
            .with_metrics(metrics.clone());
        let mut rows = 0;
        let mut sum = 0;
        for batch in reader.read().unwrap() {
            let batch = batch.unwrap();
            rows += batch.num_rows() as u64;
            sum += sum_values(&batch);
        }
        ((rows, sum), metrics.snapshot())
    }

    /// The same through the object-store reader (the ADLS decoder over an
    /// in-memory store), which counts the compressed bytes it fetches.
    fn read_object(
        store: &Arc<dyn ObjectStore>,
        name: &str,
        predicate: &StoragePredicate,
    ) -> ((u64, i64), crate::ScanMetricsSnapshot) {
        let metrics = ScanMetrics::default();
        let mut source = AdlsParquetReader::over_store(Arc::clone(store), "layout", "tests", name)
            .with_predicate(predicate.clone())
            .with_late_materialisation(LateMaterialisation::Always)
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
        let mut rows = 0;
        let mut sum = 0;
        while let Some(batch) = source.next_batch().unwrap() {
            rows += batch.num_rows() as u64;
            sum += sum_values(&batch);
        }
        ((rows, sum), metrics.snapshot())
    }

    #[test]
    fn a_clustered_layout_reads_fewer_row_groups_pages_and_bytes_than_the_same_rows_unclustered() {
        const ROWS: usize = 2_000_000;
        let plain = scattered(ROWS);
        let sorted = sorted_by_key(&plain);
        let layout =
            ClusteringLayout::new(vec!["key".into()], vec![]).with_max_row_group_rows(ROWS / 8);
        let (sink, files) = write_clustered(
            layout,
            FileNaming::Single("clustered.parquet".into()),
            &(0..ROWS)
                .step_by(65_536)
                .map(|offset| sorted.slice(offset, 65_536.min(ROWS - offset)))
                .collect::<Vec<_>>(),
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rows, ROWS as u64);
        assert_eq!(files[0].row_groups, 8);
        let clustered = sink.file("clustered.parquet").unwrap();
        let unclustered = write_plain(&plain);

        let directory = std::env::temp_dir().join(format!(
            "kaveon-clustered-skip-{}-{}",
            std::process::id(),
            mix(ROWS as u64)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let clustered_path = directory.join("clustered.parquet");
        let unclustered_path = directory.join("unclustered.parquet");
        std::fs::write(&clustered_path, &clustered).unwrap();
        std::fs::write(&unclustered_path, &unclustered).unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(store.put(&Path::from("clustered.parquet"), clustered.into()))
            .unwrap();
        runtime
            .block_on(store.put(&Path::from("unclustered.parquet"), unclustered.into()))
            .unwrap();

        for (name, predicate) in [
            ("point", point((mix(12_345) % ROWS as u64) as i64)),
            ("range", range(1_500_000, 1_510_000)),
        ] {
            let (clustered_answer, clustered_local) = read_local(&clustered_path, &predicate);
            let (unclustered_answer, unclustered_local) = read_local(&unclustered_path, &predicate);
            assert_eq!(
                clustered_answer, unclustered_answer,
                "{name}: answers differ"
            );
            assert!(clustered_answer.0 > 0, "{name}: the predicate selects rows");
            // The unsorted file: every row group can hold the key, so both
            // are read and every row examined. The clustered file: the
            // row groups whose key range excludes the predicate are never
            // opened, and the row filter examines only the survivors.
            assert_eq!(unclustered_local.row_groups_considered, 2, "{name}");
            assert_eq!(unclustered_local.row_groups_selected, 2, "{name}");
            assert_eq!(
                unclustered_local.row_filter_rows_examined, ROWS as u64,
                "{name}"
            );
            assert_eq!(clustered_local.row_groups_considered, 8, "{name}");
            assert_eq!(clustered_local.row_groups_selected, 1, "{name}");
            assert_eq!(
                clustered_local.row_filter_rows_examined,
                (ROWS / 8) as u64,
                "{name}"
            );
            assert!(
                clustered_local.compressed_bytes_selected * 4
                    < unclustered_local.compressed_bytes_selected,
                "{name}: clustered selects {} bytes, unclustered {}",
                clustered_local.compressed_bytes_selected,
                unclustered_local.compressed_bytes_selected
            );

            let (clustered_answer, clustered_object) =
                read_object(&store, "clustered.parquet", &predicate);
            let (unclustered_answer, unclustered_object) =
                read_object(&store, "unclustered.parquet", &predicate);
            assert_eq!(
                clustered_answer, unclustered_answer,
                "{name}: object answers differ"
            );
            assert_eq!(clustered_object.row_groups_selected, 1, "{name}");
            assert_eq!(unclustered_object.row_groups_selected, 2, "{name}");
            // Bytes fetched: the whole selected row group without an
            // offset index, the pages the selection touches with one.
            assert!(
                clustered_object.compressed_bytes_read * 8
                    < unclustered_object.compressed_bytes_read,
                "{name}: clustered read {} bytes, unclustered {}",
                clustered_object.compressed_bytes_read,
                unclustered_object.compressed_bytes_read
            );
            assert!(
                clustered_object.compressed_bytes_read < clustered_object.compressed_bytes_selected,
                "{name}: the offset index left {} of {} selected bytes unread",
                clustered_object.compressed_bytes_selected - clustered_object.compressed_bytes_read,
                clustered_object.compressed_bytes_selected
            );
            // The measured skip, for the record (`--nocapture`).
            eprintln!(
                "{name}: row groups {}/{} → {}/{}; rows examined {} → {}; bytes selected {} → {}; bytes read {} → {}",
                unclustered_local.row_groups_selected,
                unclustered_local.row_groups_considered,
                clustered_local.row_groups_selected,
                clustered_local.row_groups_considered,
                unclustered_local.row_filter_rows_examined,
                clustered_local.row_filter_rows_examined,
                unclustered_local.compressed_bytes_selected,
                clustered_local.compressed_bytes_selected,
                unclustered_object.compressed_bytes_read,
                clustered_object.compressed_bytes_read
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_footer_records_the_layout() {
        let sorted = sorted_by_key(&scattered(50_000));
        let layout = ClusteringLayout::new(vec!["key".into()], vec!["payload".into()])
            .with_max_row_group_rows(20_000);
        let (sink, files) = write_clustered(
            layout,
            FileNaming::Single("t.parquet".into()),
            std::slice::from_ref(&sorted),
        );
        assert_eq!(files[0].row_groups, 3);
        let bytes = sink.file("t.parquet").unwrap();
        let reader = SerializedFileReader::new(bytes::Bytes::from(bytes)).unwrap();
        let metadata = reader.metadata();
        let file_metadata = metadata.file_metadata();
        assert!(
            file_metadata
                .created_by()
                .unwrap()
                .starts_with("kaveon-storage")
        );
        let layout_entry = file_metadata
            .key_value_metadata()
            .unwrap()
            .iter()
            .find(|entry| entry.key == LAYOUT_METADATA_KEY)
            .unwrap();
        assert_eq!(layout_entry.value.as_deref(), Some("[\"key\"]"));
        for group in metadata.row_groups() {
            assert_eq!(
                group.sorting_columns().unwrap(),
                &[SortingColumn {
                    column_idx: 0,
                    descending: false,
                    nulls_first: false,
                }]
            );
            // Bloom filters on the clustering column and the declared
            // column, not on the rest; a page index on every column.
            assert!(group.column(0).bloom_filter_offset().is_some());
            assert!(group.column(1).bloom_filter_offset().is_some());
            assert!(group.column(2).bloom_filter_offset().is_none());
            for column in group.columns() {
                assert!(column.column_index_offset().is_some());
                assert!(column.offset_index_offset().is_some());
                assert!(column.statistics().is_some());
                assert!(matches!(column.compression(), Compression::ZSTD(_)));
            }
        }
        // Statistics that let a reader prune: strictly increasing key
        // ranges across the row groups.
        let bounds = metadata
            .row_groups()
            .iter()
            .map(|group| {
                let stats = group.column(0).statistics().unwrap();
                let bytes = |value: &[u8]| i64::from_le_bytes(value.try_into().unwrap());
                (
                    bytes(stats.min_bytes_opt().unwrap()),
                    bytes(stats.max_bytes_opt().unwrap()),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            bounds.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "{bounds:?}"
        );
    }

    /// A Bloom column that is not a clustering column: its values are
    /// scattered over the whole range in every row group, so the footer
    /// statistics keep all eight for a point filter; each value sits in one
    /// row group, so the Bloom filters keep one. Both readers consult them
    /// (the object reader fetches one filter per row group, the local
    /// reader reads them from the file) and count what they ruled out.
    #[test]
    fn bloom_filters_prune_the_row_groups_the_statistics_keep() {
        const ROWS: usize = 800_000;
        const GROUPS: usize = 8;
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("user", DataType::Int64, false),
            Field::new("tag", DataType::Utf8, false),
            Field::new("value", DataType::Int64, true),
        ]));
        let users = (0..ROWS as u64)
            .map(|row| mix(row) as i64)
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from((0..ROWS as i64).collect::<Vec<_>>())),
                Arc::new(Int64Array::from(users.clone())),
                Arc::new(StringArray::from_iter_values(
                    users.iter().map(|user| format!("tag-{user:x}")),
                )),
                Arc::new(Int64Array::from(
                    (0..ROWS as i64)
                        .map(|row| Some(row % 1_000))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap();
        let layout = ClusteringLayout::new(vec!["key".into()], vec!["user".into(), "tag".into()])
            .with_max_row_group_rows(ROWS / GROUPS);
        let sink = MemorySink::default();
        let mut writer = ClusteredParquetWriter::new(
            layout,
            Arc::clone(&schema),
            FileNaming::Single("bloom.parquet".into()),
            Box::new(sink.clone()),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        let files = writer.finish().unwrap();
        assert_eq!(files[0].row_groups, GROUPS);
        let bytes = sink.file("bloom.parquet").unwrap();
        let directory = std::env::temp_dir().join(format!(
            "kaveon-clustered-bloom-{}-{}",
            std::process::id(),
            mix(ROWS as u64)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("bloom.parquet");
        std::fs::write(&path, &bytes).unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(store.put(&Path::from("bloom.parquet"), bytes.into()))
            .unwrap();

        // Row 345 678 lives in row group 3; its user and tag are probed by
        // equality, alone and inside an IN and an AND.
        let probe_row = 345_678;
        let user = users[probe_row];
        let expected = (1_u64, (probe_row as i64 % 1_000));
        let user_eq = StoragePredicate::Compare {
            column: "user".into(),
            op: CompareOp::Eq,
            value: ScalarValue::Int64(user),
        };
        let tag_in = StoragePredicate::In {
            column: "tag".into(),
            values: vec![
                ScalarValue::Utf8("tag-absent".into()),
                ScalarValue::Utf8(format!("tag-{user:x}")),
            ],
        };
        let both = StoragePredicate::And(vec![
            user_eq.clone(),
            StoragePredicate::Compare {
                column: "value".into(),
                op: CompareOp::Ge,
                value: ScalarValue::Int64(0),
            },
        ]);
        for (name, predicate) in [
            ("user =", user_eq),
            ("tag IN", tag_in),
            ("user = AND", both),
        ] {
            let metrics = ScanMetrics::default();
            let reader = ParquetReader::new(&path)
                .with_predicate(predicate.clone())
                .with_metrics(metrics.clone());
            let (mut rows, mut sum) = (0_u64, 0_i64);
            for batch in reader.read().unwrap() {
                let batch = batch.unwrap();
                rows += batch.num_rows() as u64;
                sum += batch
                    .column(3)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .iter()
                    .flatten()
                    .sum::<i64>();
            }
            let local = metrics.snapshot();
            assert_eq!((rows, sum), expected, "{name}: local answer");
            assert_eq!(local.row_groups_considered, GROUPS as u64, "{name}");
            assert_eq!(local.bloom_filters_read, GROUPS as u64, "{name}");
            assert!(local.bloom_filter_bytes_read > 0, "{name}");
            assert!(
                local.row_groups_pruned_by_bloom >= GROUPS as u64 - 2,
                "{name}: Bloom filters pruned {} of {GROUPS}",
                local.row_groups_pruned_by_bloom
            );
            assert_eq!(
                local.row_groups_selected + local.row_groups_pruned_by_bloom,
                GROUPS as u64,
                "{name}: the statistics kept every row group"
            );
            assert!(
                local.row_filter_rows_examined <= 2 * (ROWS / GROUPS) as u64,
                "{name}: {} rows examined",
                local.row_filter_rows_examined
            );

            let metrics = ScanMetrics::default();
            let mut source = AdlsParquetReader::over_store(
                Arc::clone(&store),
                "layout",
                "tests",
                "bloom.parquet",
            )
            .with_predicate(predicate.clone())
            .with_late_materialisation(LateMaterialisation::Always)
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
            let mut rows = 0_u64;
            while let Some(batch) = source.next_batch().unwrap() {
                rows += batch.num_rows() as u64;
            }
            let object = metrics.snapshot();
            assert_eq!(rows, expected.0, "{name}: object answer");
            assert_eq!(
                object.row_groups_selected, local.row_groups_selected,
                "{name}"
            );
            assert_eq!(
                object.row_groups_pruned_by_bloom, local.row_groups_pruned_by_bloom,
                "{name}"
            );
            assert_eq!(object.bloom_filters_read, GROUPS as u64, "{name}");

            // The generic object-store reader (S3 and ABFSS URIs) too.
            let metrics = ScanMetrics::default();
            let mut source =
                crate::ObjectParquetReader::new(Arc::clone(&store), Path::from("bloom.parquet"))
                    .with_predicate(predicate.clone())
                    .with_metrics(metrics.clone())
                    .read_blocking()
                    .unwrap();
            let mut rows = 0_u64;
            while let Some(batch) = source.next_batch().unwrap() {
                rows += batch.num_rows() as u64;
            }
            // Its predicate is the executor's unless late materialisation
            // applies: the rows are the surviving row groups' rows.
            let generic = metrics.snapshot();
            assert_eq!(
                rows,
                generic.row_groups_selected * (ROWS / GROUPS) as u64,
                "{name}: object-store rows"
            );
            assert_eq!(
                generic.row_groups_selected, local.row_groups_selected,
                "{name}"
            );
            assert_eq!(
                generic.row_groups_pruned_by_bloom, local.row_groups_pruned_by_bloom,
                "{name}"
            );
            eprintln!(
                "{name}: statistics kept {GROUPS}/{GROUPS}, Bloom filters pruned {} ({} filters, {} bytes), rows examined {}",
                local.row_groups_pruned_by_bloom,
                local.bloom_filters_read,
                local.bloom_filter_bytes_read,
                local.row_filter_rows_examined
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn files_roll_over_at_the_byte_target_and_rows_are_checked_for_order() {
        let sorted = sorted_by_key(&scattered(120_000));
        let layout = ClusteringLayout::new(vec!["key".into()], vec![])
            .with_max_row_group_rows(10_000)
            .with_target_file_bytes(Some(64 * 1024));
        let (sink, files) = write_clustered(
            layout.clone(),
            FileNaming::Parts {
                prefix: "part-abc".into(),
            },
            &(0..120_000)
                .step_by(7_000)
                .map(|offset| sorted.slice(offset, 7_000.min(120_000 - offset)))
                .collect::<Vec<_>>(),
        );
        assert!(files.len() > 1, "{files:?}");
        assert_eq!(files.iter().map(|file| file.rows).sum::<u64>(), 120_000);
        assert_eq!(
            files
                .iter()
                .map(|file| file.name.clone())
                .collect::<Vec<_>>(),
            sink.names()
        );
        assert_eq!(files[0].name, "part-abc-00000.parquet");
        for file in &files {
            let bytes = sink.file(&file.name).unwrap();
            assert_eq!(bytes.len() as u64, file.bytes);
            let reader = SerializedFileReader::new(bytes::Bytes::from(bytes)).unwrap();
            assert_eq!(
                reader.metadata().file_metadata().num_rows() as u64,
                file.rows
            );
            assert_eq!(reader.metadata().num_row_groups(), file.row_groups);
        }

        // A batch that breaks the order fails by row, before any of it is
        // written; the same rows unclustered are accepted.
        let sink = MemorySink::default();
        let mut writer = ClusteredParquetWriter::new(
            layout,
            schema(),
            FileNaming::Single("t.parquet".into()),
            Box::new(sink),
        )
        .unwrap();
        writer.write(&sorted.slice(0, 1_000)).unwrap();
        let error = writer.write(&sorted.slice(0, 10)).unwrap_err().to_string();
        assert!(error.contains("not in clustering order"), "{error}");
        assert!(error.contains("row 1000"), "{error}");
        let unsorted = scattered(1_000);
        let error = writer.write(&unsorted).unwrap_err().to_string();
        assert!(error.contains("not in clustering order"), "{error}");
        let mut compaction = ClusteredParquetWriter::new(
            ClusteringLayout::new(vec![], vec![]),
            schema(),
            FileNaming::Single("t.parquet".into()),
            Box::new(MemorySink::default()),
        )
        .unwrap();
        compaction.write(&unsorted).unwrap();
        assert_eq!(compaction.finish().unwrap()[0].rows, 1_000);

        // No rows, no file.
        let sink = MemorySink::default();
        let writer = ClusteredParquetWriter::new(
            ClusteringLayout::new(vec!["key".into()], vec![]),
            schema(),
            FileNaming::Single("t.parquet".into()),
            Box::new(sink.clone()),
        )
        .unwrap();
        assert!(writer.finish().unwrap().is_empty());
        assert!(sink.names().is_empty());

        // The layout names columns of the schema.
        assert!(
            ClusteredParquetWriter::new(
                ClusteringLayout::new(vec!["missing".into()], vec![]),
                schema(),
                FileNaming::Single("t.parquet".into()),
                Box::new(MemorySink::default()),
            )
            .is_err()
        );
    }
}
