use arrow::datatypes::{DataType, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchReader as ArrowRecordBatchReader};
use kaveon_core::{BatchSource, CompareOp, KaveonError, Result, ScalarValue, StoragePredicate};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};
use parquet::file::metadata::{ParquetMetaData, RowGroupMetaData};
use parquet::file::statistics::Statistics;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::scan_predicate::RowFilterPlan;
use crate::{ScanMetrics, ScanPartition};

const DEFAULT_BATCH_SIZE: usize = 8_192;

#[derive(Clone, Debug)]
pub struct ParquetFileMetadata {
    pub schema: SchemaRef,
    pub row_count: u64,
    pub row_group_count: usize,
    /// Column-chunk statistics and sizes from the footer(s), merged over
    /// row groups and files.
    pub profile: crate::FooterProfile,
}

/// Streaming adapter over parquet-rs that implements the shared execution
/// contract while also remaining usable as a standard iterator. One file, or
/// the files of a directory table one after another.
pub struct ParquetBatchIterator {
    schema: SchemaRef,
    inner: ParquetBatchInner,
    metrics: ScanMetrics,
    output_projection: Option<Vec<usize>>,
}

enum ParquetBatchInner {
    File(ParquetRecordBatchReader),
    Directory(Box<DirectoryFiles>),
}

/// The files of a local directory table this scan reads, opened lazily in
/// listing order; each is checked against the first listed file's schema as
/// it is opened.
struct DirectoryFiles {
    root: PathBuf,
    first: String,
    file_schema: SchemaRef,
    /// (absolute path, listing-relative path, whether the partition applies
    /// inside the file).
    files: Vec<(PathBuf, String, bool)>,
    next: usize,
    current: Option<ParquetBatchIterator>,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
}

impl DirectoryFiles {
    fn open_next(&mut self, metrics: &ScanMetrics) -> Result<bool> {
        let Some((path, relative, split)) = self.files.get(self.next) else {
            return Ok(false);
        };
        self.next += 1;
        let mut reader = ParquetReader::new(path)
            .with_batch_size(self.batch_size)
            .with_metrics(metrics.clone());
        if let Some(columns) = &self.columns {
            reader = reader.with_columns(columns.clone());
        }
        if let Some(predicate) = &self.predicate {
            reader = reader.with_predicate(predicate.clone());
        }
        if *split && let Some(partition) = self.partition {
            reader = reader.with_partition(partition);
        }
        let footer_started = Instant::now();
        let builder = reader.open_builder()?;
        metrics.footer_time(footer_started.elapsed());
        crate::parquet_directory::check_file_schema(
            &self.root.display().to_string(),
            &self.first,
            &self.file_schema,
            relative,
            builder.schema(),
        )?;
        metrics.file_opened();
        self.current = Some(reader.finish(builder, metrics.clone())?);
        Ok(true)
    }
}

impl Iterator for ParquetBatchIterator {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            ParquetBatchInner::File(inner) => {
                let started = Instant::now();
                let result = inner.next();
                self.metrics.read_time(started.elapsed());
                result.map(|result| {
                    result
                        .inspect(|batch| {
                            self.metrics.emitted(batch.num_rows());
                        })
                        .map_err(|error| storage_error(error.to_string()))
                        .and_then(|batch| match &self.output_projection {
                            Some(indices) => batch
                                .project(indices)
                                .map_err(|e| storage_error(e.to_string())),
                            None => Ok(batch),
                        })
                })
            }
            ParquetBatchInner::Directory(directory) => loop {
                if let Some(current) = directory.current.as_mut() {
                    match current.next() {
                        Some(Ok(batch)) => {
                            return Some(
                                RecordBatch::try_new(
                                    Arc::clone(&self.schema),
                                    batch.columns().to_vec(),
                                )
                                .map_err(|error| storage_error(error.to_string())),
                            );
                        }
                        Some(Err(error)) => return Some(Err(error)),
                        None => directory.current = None,
                    }
                }
                match directory.open_next(&self.metrics) {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(error) => return Some(Err(error)),
                }
            },
        }
    }
}

impl BatchSource for ParquetBatchIterator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.next().transpose()
    }
}

impl ParquetBatchIterator {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

/// Configuration for a synchronous local Parquet read: one file, or a
/// directory of files.
pub struct ParquetReader {
    path: PathBuf,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    metrics: Option<ScanMetrics>,
    partition: Option<ScanPartition>,
    listing: Option<Arc<crate::DirectoryListing>>,
}

impl ParquetReader {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            metrics: None,
            partition: None,
            listing: None,
        }
    }

    /// Read a directory table at a listing already taken for this query.
    pub fn with_listing(mut self, listing: Arc<crate::DirectoryListing>) -> Self {
        self.listing = Some(listing);
        self
    }

    /// The pinned listing, or the directory listed now.
    fn listing(&self) -> Result<Arc<crate::DirectoryListing>> {
        match &self.listing {
            Some(listing) => Ok(Arc::clone(listing)),
            None => local_directory_listing(&self.path).map(Arc::new),
        }
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    pub fn with_predicate(mut self, predicate: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(existing) => StoragePredicate::And(vec![existing, predicate]),
            None => predicate,
        });
        self
    }

    pub fn with_partition(mut self, partition: ScanPartition) -> Self {
        self.partition = Some(partition);
        self
    }

    pub fn with_metrics(mut self, metrics: ScanMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn read(&self) -> Result<ParquetBatchIterator> {
        if self.batch_size == 0 {
            return Err(storage_error("batch size must be greater than zero"));
        }
        let metrics = self.metrics.clone().unwrap_or_default();
        if self.path.is_dir() {
            return self.read_directory(metrics);
        }
        metrics.files_considered(1);
        let footer_started = Instant::now();
        let builder = self.open_builder()?;
        metrics.footer_time(footer_started.elapsed());
        metrics.file_opened();
        self.finish(builder, metrics)
    }

    /// The scan over an opened footer: projection, pruning, the partition's
    /// row groups, and the caller's column order.
    fn finish(
        &self,
        builder: ParquetRecordBatchReaderBuilder<File>,
        metrics: ScanMetrics,
    ) -> Result<ParquetBatchIterator> {
        let builder = self.configure_builder(builder, &metrics)?;
        let inner = builder.build().map_err(parquet_error)?;
        let (schema, output_projection) =
            ordered_projection(inner.schema(), self.columns.as_deref())?;
        Ok(ParquetBatchIterator {
            schema,
            inner: ParquetBatchInner::File(inner),
            metrics,
            output_projection,
        })
    }

    /// The path read as a directory table: the listing rule, the file
    /// assignment and the schema check are those of the object-store path.
    fn read_directory(&self, metrics: ScanMetrics) -> Result<ParquetBatchIterator> {
        let listing_started = Instant::now();
        let listing = self.listing()?;
        metrics.snapshot_time(listing_started.elapsed());
        let Some(first) = listing.files.first() else {
            return Err(storage_error(format!(
                "directory '{}' holds no Parquet data files",
                self.path.display()
            )));
        };
        let assignment = match self.partition {
            Some(partition) => crate::parquet_directory::assign_files(&listing.sizes(), partition),
            None => crate::FileAssignment {
                whole: (0..listing.files.len()).collect(),
                split: Vec::new(),
            },
        };
        metrics.files_considered(assignment.len() as u64);
        let file_schema = ParquetReader::new(self.path.join(first.path.as_ref()))
            .metadata()?
            .schema;
        let schema =
            crate::parquet_directory::advertised_schema(&file_schema, self.columns.as_deref())?;
        let files = assignment
            .files()
            .into_iter()
            .map(|(index, split)| {
                let relative = listing.files[index].path.to_string();
                (self.path.join(&relative), relative, split)
            })
            .collect();
        Ok(ParquetBatchIterator {
            schema,
            inner: ParquetBatchInner::Directory(Box::new(DirectoryFiles {
                root: self.path.clone(),
                first: first.path.to_string(),
                file_schema,
                files,
                next: 0,
                current: None,
                batch_size: self.batch_size,
                columns: self.columns.clone(),
                predicate: self.predicate.clone(),
                partition: self.partition,
            })),
            metrics,
            output_projection: None,
        })
    }

    /// Convenience method for callers that explicitly want materialization.
    pub fn read_batches(&self) -> Result<Vec<RecordBatch>> {
        self.read()?.collect()
    }

    /// Exact metadata: one file's, or a directory table's summed over every
    /// file with each checked against the first listed file's schema.
    pub fn metadata(&self) -> Result<ParquetFileMetadata> {
        if self.path.is_dir() {
            return self.directory_metadata();
        }
        let builder = self.open_builder()?;
        let row_count = u64::try_from(builder.metadata().file_metadata().num_rows())
            .map_err(|_| storage_error("Parquet metadata contains a negative row count"))?;
        let file = std::fs::metadata(&self.path)?;
        Ok(ParquetFileMetadata {
            schema: Arc::clone(builder.schema()),
            row_count,
            row_group_count: builder.metadata().num_row_groups(),
            profile: crate::FooterProfile::from_parquet(
                builder.metadata(),
                file.len(),
                file.modified().ok().and_then(|modified| {
                    modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|since| i64::try_from(since.as_millis()).ok())
                }),
            ),
        })
    }

    fn directory_metadata(&self) -> Result<ParquetFileMetadata> {
        let listing = self.listing()?;
        let Some(first) = listing.files.first() else {
            return Err(storage_error(format!(
                "directory '{}' holds no Parquet data files",
                self.path.display()
            )));
        };
        let mut combined = ParquetReader::new(self.path.join(first.path.as_ref())).metadata()?;
        for file in listing.files.iter().skip(1) {
            let next = ParquetReader::new(self.path.join(file.path.as_ref())).metadata()?;
            crate::parquet_directory::check_file_schema(
                &self.path.display().to_string(),
                first.path.as_ref(),
                &combined.schema,
                file.path.as_ref(),
                &next.schema,
            )?;
            combined.row_count = combined
                .row_count
                .checked_add(next.row_count)
                .ok_or_else(|| storage_error("Parquet directory row count overflow"))?;
            combined.row_group_count =
                combined
                    .row_group_count
                    .checked_add(next.row_group_count)
                    .ok_or_else(|| storage_error("Parquet directory row-group count overflow"))?;
            combined.profile.merge(next.profile);
        }
        Ok(combined)
    }

    fn open_builder(&self) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        if self.batch_size == 0 {
            return Err(storage_error("batch size must be greater than zero"));
        }
        // With a predicate the decoder runs a row filter; the offset index,
        // when the file carries one, lets it skip whole pages the selection
        // never touches instead of decompressing them.
        let options = ArrowReaderOptions::new().with_page_index(self.predicate.is_some());
        // The path is the fact a reader of the error needs: a table whose
        // file moved answers "cannot open /data/x.parquet", not "os error 2".
        let file = File::open(&self.path).map_err(|error| {
            storage_error(format!(
                "cannot open '{}': {}",
                self.path.display(),
                io_reason(&error)
            ))
        })?;
        ParquetRecordBatchReaderBuilder::try_new_with_options(file, options).map_err(parquet_error)
    }

    fn configure_builder(
        &self,
        mut builder: ParquetRecordBatchReaderBuilder<File>,
        metrics: &ScanMetrics,
    ) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        let schema = Arc::clone(builder.schema());
        builder = builder.with_batch_size(self.batch_size);

        let projection = self
            .columns
            .as_ref()
            .map(|columns| projection_indices(&schema, columns))
            .transpose()?;

        if let Some(projection) = projection.as_ref() {
            let mask = ProjectionMask::roots(builder.parquet_schema(), projection.clone());
            builder = builder.with_projection(mask);
        }

        // Literals meet their columns' types here, once the schema is known.
        let coerced = self
            .predicate
            .as_ref()
            .map(|predicate| predicate.coerced_for(&schema));
        let mut groups = if let Some(predicate) = &coerced {
            validate_predicate(predicate, &schema)?;
            matching_row_groups(builder.metadata().as_ref(), &schema, predicate)
        } else {
            (0..builder.metadata().num_row_groups()).collect()
        };
        if let Some(partition) = self.partition {
            groups.retain(|group| partition.contains(*group));
        }
        record_selection_metrics(
            builder.metadata().as_ref(),
            &groups,
            projection.as_deref(),
            metrics,
        );
        // The evaluable part of the predicate runs inside the decoder: the
        // rows it rejects are never decoded for the other projected columns.
        // A local file costs nothing to read twice, so every evaluable
        // predicate is a row filter here.
        if let Some(mut plan) = coerced
            .as_ref()
            .and_then(|predicate| RowFilterPlan::new(predicate, &schema))
        {
            plan.order_by_bytes(builder.metadata().as_ref(), &groups);
            let row_filter = plan.row_filter(builder.parquet_schema(), metrics);
            builder = builder.with_row_filter(row_filter);
        }
        if self.predicate.is_some() || self.partition.is_some() {
            builder = builder.with_row_groups(groups);
        }
        Ok(builder)
    }
}

pub(crate) fn record_selection_metrics(
    metadata: &ParquetMetaData,
    groups: &[usize],
    projection: Option<&[usize]>,
    metrics: &ScanMetrics,
) {
    metrics.row_groups(metadata.num_row_groups() as u64, groups.len() as u64);
    let mut rows = 0_u64;
    let mut bytes = 0_u64;
    for index in groups {
        let group = metadata.row_group(*index);
        rows = rows.saturating_add(group.num_rows().max(0) as u64);
        for (column_index, column) in group.columns().iter().enumerate() {
            if projection.is_none_or(|indices| indices.contains(&column_index)) {
                bytes = bytes.saturating_add(column.compressed_size().max(0) as u64);
            }
        }
    }
    metrics.selected(rows, bytes);
}

/// Parquet ProjectionMask selects columns in physical file order. Reorder the
/// decoded arrays and advertised schema together to honor requested scan order.
pub(crate) fn ordered_projection(
    schema: SchemaRef,
    columns: Option<&[String]>,
) -> Result<(SchemaRef, Option<Vec<usize>>)> {
    let Some(columns) = columns else {
        return Ok((schema, None));
    };
    let indices = projection_indices(&schema, columns)?;
    if indices.iter().copied().eq(0..schema.fields().len()) {
        return Ok((schema, None));
    }
    let output = Arc::new(
        schema
            .project(&indices)
            .map_err(|e| storage_error(e.to_string()))?,
    );
    Ok((output, Some(indices)))
}

pub(crate) fn projection_indices(schema: &SchemaRef, columns: &[String]) -> Result<Vec<usize>> {
    if columns.is_empty() {
        return Err(storage_error("projection must contain at least one column"));
    }
    let mut seen = HashSet::with_capacity(columns.len());
    columns
        .iter()
        .map(|column| {
            if !seen.insert(column.as_str()) {
                return Err(storage_error(format!(
                    "projection contains duplicate column '{column}'"
                )));
            }
            schema.index_of(column).map_err(|_| {
                storage_error(format!("projection references unknown column '{column}'"))
            })
        })
        .collect()
}

pub(crate) fn validate_predicate(predicate: &StoragePredicate, schema: &SchemaRef) -> Result<()> {
    match predicate {
        StoragePredicate::Compare { column, value, .. } => {
            validate_column_value(column, value, schema)
        }
        StoragePredicate::IsNull { column }
        | StoragePredicate::IsNotNull { column }
        | StoragePredicate::Like { column, .. } => validate_column(column, schema).map(|_| ()),
        StoragePredicate::In { column, values } => {
            validate_column(column, schema)?;
            for value in values {
                validate_column_value(column, value, schema)?;
            }
            Ok(())
        }
        StoragePredicate::And(predicates) | StoragePredicate::Or(predicates) => {
            for child in predicates {
                validate_predicate(child, schema)?;
            }
            Ok(())
        }
        StoragePredicate::Not(predicate) => validate_predicate(predicate, schema),
    }
}

fn validate_column<'a>(column: &str, schema: &'a SchemaRef) -> Result<&'a DataType> {
    schema
        .field_with_name(column)
        .map(|field| field.data_type())
        .map_err(|_| storage_error(format!("predicate references unknown column '{column}'")))
}

fn validate_column_value(column: &str, value: &ScalarValue, schema: &SchemaRef) -> Result<()> {
    let data_type = validate_column(column, schema)?;
    if matches!(value, ScalarValue::Null) {
        return Err(storage_error(format!(
            "comparison predicate for column '{column}' cannot use NULL"
        )));
    }
    if !scalar_matches_data_type(value, data_type) {
        return Err(storage_error(format!(
            "predicate value type does not match column '{column}' ({data_type})"
        )));
    }
    Ok(())
}

pub(crate) fn matching_row_groups(
    metadata: &ParquetMetaData,
    schema: &SchemaRef,
    predicate: &StoragePredicate,
) -> Vec<usize> {
    metadata
        .row_groups()
        .iter()
        .enumerate()
        .filter_map(|(index, group)| predicate_can_match(group, schema, predicate).then_some(index))
        .collect()
}

fn predicate_can_match(
    group: &RowGroupMetaData,
    schema: &SchemaRef,
    predicate: &StoragePredicate,
) -> bool {
    match predicate {
        StoragePredicate::Compare { column, op, value } => schema
            .index_of(column)
            .ok()
            .is_none_or(|index| compare_can_match(group, index, *op, value)),
        StoragePredicate::IsNull { column } => column_statistics(group, schema, column)
            .is_none_or(|stats| stats.null_count_opt() != Some(0)),
        StoragePredicate::IsNotNull { column } => column_statistics(group, schema, column)
            .is_none_or(|stats| stats.null_count_opt() != Some(group.num_rows() as u64)),
        StoragePredicate::In { column, values } => {
            schema.index_of(column).ok().is_none_or(|index| {
                values
                    .iter()
                    .any(|value| compare_can_match(group, index, CompareOp::Eq, value))
            })
        }
        StoragePredicate::And(predicates) => predicates
            .iter()
            .all(|child| predicate_can_match(group, schema, child)),
        StoragePredicate::Or(predicates) => predicates
            .iter()
            .any(|child| predicate_can_match(group, schema, child)),
        // A "may match" result cannot be safely inverted. Retaining the row
        // group preserves correctness until exact domain reasoning is added.
        StoragePredicate::Not(_) => true,
        // Statistics say nothing about a pattern match; the row filter does.
        StoragePredicate::Like { .. } => true,
    }
}

fn column_statistics<'a>(
    group: &'a RowGroupMetaData,
    schema: &SchemaRef,
    column: &str,
) -> Option<&'a Statistics> {
    schema
        .index_of(column)
        .ok()
        .and_then(|index| group.column(index).statistics())
}

fn compare_can_match(
    group: &RowGroupMetaData,
    column: usize,
    op: CompareOp,
    value: &ScalarValue,
) -> bool {
    let Some(stats) = group.column(column).statistics() else {
        return true;
    };
    // Inexact bounds are still bounds. Parquet lets a writer truncate long
    // byte-array statistics, and the format requires a truncated minimum to
    // sit at or below the true minimum and a truncated maximum to be raised
    // above the true maximum, so range reasoning on them stays sound. Most
    // writers, pyarrow included, never set the exactness flags at all, and
    // requiring them here disabled pruning on every string column such files
    // carry. Only the deprecated min/max fields, whose byte ordering is
    // undefined, force the conservative path.
    if stats.is_min_max_deprecated() {
        return true;
    }
    match stats {
        Statistics::Boolean(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Bool),
            stats.max_opt().copied().map(ScalarValue::Bool),
            op,
            value,
        ),
        Statistics::Int32(stats) => bounds_can_match(
            stats
                .min_opt()
                .map(|value| ScalarValue::Int64(i64::from(*value))),
            stats
                .max_opt()
                .map(|value| ScalarValue::Int64(i64::from(*value))),
            op,
            value,
        ),
        Statistics::Int64(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Int64),
            stats.max_opt().copied().map(ScalarValue::Int64),
            op,
            value,
        ),
        Statistics::Float(stats) => bounds_can_match(
            stats
                .min_opt()
                .map(|value| ScalarValue::Float64(f64::from(*value))),
            stats
                .max_opt()
                .map(|value| ScalarValue::Float64(f64::from(*value))),
            op,
            value,
        ),
        Statistics::Double(stats) => bounds_can_match(
            stats.min_opt().copied().map(ScalarValue::Float64),
            stats.max_opt().copied().map(ScalarValue::Float64),
            op,
            value,
        ),
        Statistics::ByteArray(stats) => {
            let min = stats
                .min_opt()
                .and_then(|value| std::str::from_utf8(value.data()).ok())
                .map(|value| ScalarValue::Utf8(value.to_owned()));
            let max = stats
                .max_opt()
                .and_then(|value| std::str::from_utf8(value.data()).ok())
                .map(|value| ScalarValue::Utf8(value.to_owned()));
            bounds_can_match(min, max, op, value)
        }
        Statistics::Int96(_) | Statistics::FixedLenByteArray(_) => true,
    }
}

fn bounds_can_match(
    min: Option<ScalarValue>,
    max: Option<ScalarValue>,
    op: CompareOp,
    value: &ScalarValue,
) -> bool {
    let (Some(min), Some(max)) = (min, max) else {
        return true;
    };
    match op {
        CompareOp::Eq => within_bounds(&min, &max, value),
        CompareOp::Ne => {
            !(scalar_cmp(&min, value) == Some(Ordering::Equal)
                && scalar_cmp(&max, value) == Some(Ordering::Equal))
        }
        CompareOp::Lt => scalar_cmp(&min, value) == Some(Ordering::Less),
        CompareOp::Le => scalar_cmp(&min, value) != Some(Ordering::Greater),
        CompareOp::Gt => scalar_cmp(&max, value) == Some(Ordering::Greater),
        CompareOp::Ge => scalar_cmp(&max, value) != Some(Ordering::Less),
    }
}

fn within_bounds(min: &ScalarValue, max: &ScalarValue, value: &ScalarValue) -> bool {
    scalar_cmp(min, value) != Some(Ordering::Greater)
        && scalar_cmp(max, value) != Some(Ordering::Less)
}

fn scalar_matches_data_type(value: &ScalarValue, data_type: &DataType) -> bool {
    // A dictionary column is its value type for every comparison.
    let data_type = match data_type {
        DataType::Dictionary(_, values) => values.as_ref(),
        other => other,
    };
    matches!(
        (value, data_type),
        (ScalarValue::Bool(_), DataType::Boolean)
            | (
                ScalarValue::Int64(_),
                DataType::Int8
                    | DataType::Int16
                    | DataType::Int32
                    | DataType::Int64
                    | DataType::UInt8
                    | DataType::UInt16
                    | DataType::UInt32
                    | DataType::UInt64
                    | DataType::Date32
            )
            | (
                ScalarValue::Float64(_),
                DataType::Float32 | DataType::Float64
            )
            | (ScalarValue::Utf8(_), DataType::Utf8 | DataType::LargeUtf8)
    )
}

fn scalar_cmp(left: &ScalarValue, right: &ScalarValue) -> Option<Ordering> {
    match (left, right) {
        (ScalarValue::Bool(left), ScalarValue::Bool(right)) => left.partial_cmp(right),
        (ScalarValue::Int64(left), ScalarValue::Int64(right)) => left.partial_cmp(right),
        (ScalarValue::Float64(left), ScalarValue::Float64(right)) => left.partial_cmp(right),
        (ScalarValue::Utf8(left), ScalarValue::Utf8(right)) => left.partial_cmp(right),
        _ => None,
    }
}

/// The data files under a local directory, through the same listing rule as
/// an object store: paths are relative to the directory, sorted.
pub(crate) fn local_directory_listing(
    directory: &Path,
) -> Result<crate::parquet_directory::DirectoryListing> {
    let store = Arc::new(
        object_store::local::LocalFileSystem::new_with_prefix(directory)
            .map_err(|error| storage_error(error.to_string()))?,
    );
    crate::delta_snapshot::blocking(async move {
        crate::parquet_directory::list_parquet_directory(
            store.as_ref(),
            &object_store::path::Path::default(),
        )
        .await
    })
}

fn storage_error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

/// An I/O error's reason without the platform's `(os error N)` suffix.
fn io_reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error") {
        Some(index) => text[..index].to_owned(),
        None => text,
    }
}

fn parquet_error(error: parquet::errors::ParquetError) -> KaveonError {
    storage_error(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Int32Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    const ROW_GROUP_SIZE: usize = 3;
    static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn fixture() -> TestFile {
        let id = NEXT_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let file = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-{}-{id}.parquet",
            std::process::id()
        )));
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("label", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(vec![0, 1, 2, 3, 4, 5])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                    Some("f"),
                ])) as ArrayRef,
            ],
        )
        .expect("test batch must be valid");
        let properties = WriterProperties::builder()
            .set_max_row_group_size(ROW_GROUP_SIZE)
            .build();
        let output = File::create(&file.0).expect("test file must be creatable");
        let mut writer = ArrowWriter::try_new(output, schema, Some(properties))
            .expect("test writer must be valid");
        writer.write(&batch).expect("test batch must be writable");
        writer.close().expect("test writer must close");
        file
    }

    fn compare(column: &str, op: CompareOp, value: ScalarValue) -> StoragePredicate {
        StoragePredicate::Compare {
            column: column.to_owned(),
            op,
            value,
        }
    }

    fn row_count(reader: &ParquetReader) -> usize {
        reader
            .read()
            .expect("reader must open")
            .map(|batch| batch.expect("batch must decode").num_rows())
            .sum()
    }

    #[test]
    fn reports_file_metadata() {
        let file = fixture();
        let metadata = ParquetReader::new(&file.0)
            .metadata()
            .expect("metadata must load");
        assert_eq!(metadata.row_count, 6);
        assert_eq!(metadata.row_group_count, 2);
        assert_eq!(metadata.schema.fields().len(), 2);
    }

    #[test]
    fn reversed_projection_agrees_for_empty_and_nonempty_partitions() {
        let file = fixture();
        let mut seen = Vec::new();
        let mut schema = None;
        for partition in 0..3 {
            let mut source = ParquetReader::new(&file.0)
                .with_columns(vec!["label".into(), "id".into()])
                .with_partition(ScanPartition::new(partition, 3).unwrap())
                .read()
                .unwrap();
            assert_eq!(source.schema().field(0).name(), "label");
            assert_eq!(source.schema().field(1).name(), "id");
            if let Some(schema) = &schema {
                assert_eq!(source.schema(), schema);
            } else {
                schema = Some(source.schema().clone());
            }
            while let Some(batch) = source.next_batch().unwrap() {
                assert_eq!(batch.schema(), schema.clone().unwrap());
                seen.extend(
                    batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<Int32Array>()
                        .unwrap()
                        .values()
                        .iter()
                        .copied(),
                );
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn streams_batches_through_shared_contract() {
        let file = fixture();
        let mut source = ParquetReader::new(&file.0)
            .with_batch_size(2)
            .read()
            .expect("reader must open");
        assert_eq!(source.schema().fields().len(), 2);
        let mut rows = 0;
        while let Some(batch) = source.next_batch().expect("batch must decode") {
            assert!(batch.num_rows() <= 2);
            rows += batch.num_rows();
        }
        assert_eq!(rows, 6);
    }

    #[test]
    fn projects_columns_and_rejects_invalid_projection() {
        let file = fixture();
        let mut source = ParquetReader::new(&file.0)
            .with_columns(vec!["label".to_owned()])
            .read()
            .expect("projection must read");
        assert_eq!(source.schema().fields().len(), 1);
        assert_eq!(source.schema().field(0).name(), "label");
        let batches = source
            .by_ref()
            .collect::<Result<Vec<_>>>()
            .expect("projection must decode");
        assert!(batches.iter().all(|batch| batch.num_columns() == 1));
        assert_eq!(batches[0].schema().field(0).name(), "label");

        for columns in [
            vec!["missing".to_owned()],
            vec!["id".to_owned(), "id".to_owned()],
            Vec::new(),
        ] {
            assert!(
                ParquetReader::new(&file.0)
                    .with_columns(columns)
                    .read()
                    .is_err()
            );
        }
    }

    #[test]
    fn validates_configuration_and_predicates() {
        let file = fixture();
        assert!(
            ParquetReader::new(&file.0)
                .with_batch_size(0)
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(StoragePredicate::IsNull {
                    column: "missing".to_owned()
                })
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(compare(
                    "id",
                    CompareOp::Eq,
                    ScalarValue::Utf8("1".to_owned())
                ))
                .read()
                .is_err()
        );
        assert!(
            ParquetReader::new(&file.0)
                .with_predicate(compare("id", CompareOp::Eq, ScalarValue::Null))
                .read()
                .is_err()
        );
    }

    #[test]
    fn reports_missing_and_corrupt_files() {
        let missing = std::env::temp_dir().join("kaveon-storage-file-does-not-exist.parquet");
        let Err(error) = ParquetReader::new(&missing).read() else {
            panic!("a missing file must not read");
        };
        assert!(matches!(error, KaveonError::Storage(_)));
        let text = error.to_string();
        assert!(
            text.contains(&format!("cannot open '{}'", missing.display())),
            "{text}"
        );
        assert!(!text.contains("os error"), "{text}");

        let corrupt = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-corrupt-{}.parquet",
            std::process::id()
        )));
        std::fs::write(&corrupt.0, b"not a parquet file").expect("corrupt fixture must write");
        assert!(matches!(
            ParquetReader::new(&corrupt.0).read(),
            Err(KaveonError::Storage(_))
        ));
    }

    #[test]
    fn prunes_on_byte_array_bounds_without_exactness_flags() {
        use parquet::basic::Type as PhysicalType;
        use parquet::file::metadata::{ColumnChunkMetaData, RowGroupMetaData};
        use parquet::file::statistics::ValueStatistics;
        use parquet::schema::types::{SchemaDescriptor, Type as SchemaType};

        // A row group whose event_date statistics were written the way
        // pyarrow writes them: min/max present, exactness flags absent.
        let column = SchemaType::primitive_type_builder("event_date", PhysicalType::BYTE_ARRAY)
            .build()
            .unwrap();
        let root = SchemaType::group_type_builder("schema")
            .with_fields(vec![Arc::new(column)])
            .build()
            .unwrap();
        let descriptor = Arc::new(SchemaDescriptor::new(Arc::new(root)));
        let statistics = Statistics::ByteArray(
            ValueStatistics::new(
                Some("2026-07-04".into()),
                Some("2026-07-04".into()),
                None,
                Some(0),
                false,
            )
            .with_min_is_exact(false)
            .with_max_is_exact(false),
        );
        let chunk = ColumnChunkMetaData::builder(descriptor.column(0))
            .set_statistics(statistics)
            .set_num_values(3_000_000)
            .build()
            .unwrap();
        let group = RowGroupMetaData::builder(descriptor)
            .set_num_rows(3_000_000)
            .set_column_metadata(vec![chunk])
            .build()
            .unwrap();

        let in_2025 = ScalarValue::Utf8("2025-01-01".into());
        let in_2027 = ScalarValue::Utf8("2027-01-01".into());
        let the_day = ScalarValue::Utf8("2026-07-04".into());
        // event_date < '2026-01-01' cannot match a group whose min is 2026-07-04
        assert!(!compare_can_match(
            &group,
            0,
            CompareOp::Lt,
            &ScalarValue::Utf8("2026-01-01".into())
        ));
        assert!(!compare_can_match(&group, 0, CompareOp::Eq, &in_2025));
        assert!(!compare_can_match(&group, 0, CompareOp::Ge, &in_2027));
        assert!(compare_can_match(&group, 0, CompareOp::Eq, &the_day));
        assert!(compare_can_match(&group, 0, CompareOp::Ge, &in_2025));
    }

    #[test]
    fn prunes_row_groups_and_filters_decoded_int64_rows() {
        let file = fixture();
        let matching = ParquetReader::new(&file.0).with_predicate(compare(
            "id",
            CompareOp::Eq,
            ScalarValue::Int64(4),
        ));
        // The fixture stores id as Int32: the literal is cast to the column's
        // width, so the decoded rows are filtered exactly, not only pruned by
        // row group.
        assert_eq!(row_count(&matching), 1);

        let absent = ParquetReader::new(&file.0).with_predicate(compare(
            "id",
            CompareOp::Eq,
            ScalarValue::Int64(99),
        ));
        assert_eq!(row_count(&absent), 0);
    }

    #[test]
    fn pushes_int64_comparison_into_decoder_without_exposing_filter_column() {
        let id = NEXT_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let file = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-int64-filter-{}-{id}.parquet",
            std::process::id()
        )));
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("label", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(arrow::array::Int64Array::from(vec![
                    Some(0),
                    Some(1),
                    None,
                    Some(3),
                    Some(4),
                    Some(5),
                ])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e", "f"])),
            ],
        )
        .unwrap();
        let output = File::create(&file.0).unwrap();
        let mut writer = ArrowWriter::try_new(output, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let mut reader = ParquetReader::new(&file.0)
            .with_columns(vec!["label".to_owned()])
            .with_predicate(compare("id", CompareOp::Gt, ScalarValue::Int64(3)))
            .read()
            .unwrap();
        let metrics = reader.metrics();
        let batches = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        assert_eq!(batches[0].num_columns(), 1);
        assert_eq!(batches[0].schema().field(0).name(), "label");
        let metrics = metrics.snapshot();
        assert_eq!(metrics.rows_selected, 6);
        assert_eq!(metrics.rows_emitted, 2);
    }

    #[test]
    fn pushes_utf8_and_conjoined_comparisons_into_decoder() {
        let id = NEXT_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let file = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-utf8-filter-{}-{id}.parquet",
            std::process::id()
        )));
        let schema = Arc::new(Schema::new(vec![
            Field::new("day", DataType::Utf8, false),
            Field::new("amount", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec![
                    "2026-07-01",
                    "2026-07-02",
                    "2026-07-03",
                    "2026-08-01",
                    "2026-08-02",
                    "2026-09-01",
                ])),
                Arc::new(arrow::array::Int64Array::from(vec![1, 2, 3, 4, 5, 6])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e", "f"])),
            ],
        )
        .unwrap();
        let output = File::create(&file.0).unwrap();
        let mut writer = ArrowWriter::try_new(output, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        // day >= '2026-07-02' AND day < '2026-09-01' AND amount > 2: three
        // decoder predicates; only the surviving rows' labels are decoded.
        let predicate = StoragePredicate::And(vec![
            compare("day", CompareOp::Ge, ScalarValue::Utf8("2026-07-02".into())),
            compare("day", CompareOp::Lt, ScalarValue::Utf8("2026-09-01".into())),
            compare("amount", CompareOp::Gt, ScalarValue::Int64(2)),
        ]);
        let arrow_schema = Arc::new(Schema::new(vec![
            Field::new("day", DataType::Utf8, false),
            Field::new("amount", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let plan = RowFilterPlan::new(&predicate, &arrow_schema).unwrap();
        assert_eq!(plan.columns(), vec![0, 1]);

        let metrics = ScanMetrics::default();
        let mut reader = ParquetReader::new(&file.0)
            .with_columns(vec!["label".to_owned()])
            .with_predicate(predicate)
            .with_metrics(metrics.clone())
            .read()
            .unwrap();
        let batches = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        let labels: Vec<String> = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .iter()
                    .map(|value| value.unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(labels, vec!["c", "d", "e"]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.row_filter_rows_examined, 6);
        assert_eq!(snapshot.row_filter_rows_admitted, 3);

        // A dictionary-typed column (the Arrow schema stored in the file)
        // takes the same path: the kernel compares the dictionary once.
        let dictionary_file = TestFile(std::env::temp_dir().join(format!(
            "kaveon-storage-dict-filter-{}-{id}.parquet",
            std::process::id()
        )));
        let dictionary_type =
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let dictionary_schema = Arc::new(Schema::new(vec![
            Field::new("day", dictionary_type.clone(), false),
            Field::new("amount", DataType::Int64, false),
        ]));
        let days: arrow::array::DictionaryArray<arrow::datatypes::Int32Type> =
            vec!["2026-07-01", "2026-08-01", "2026-09-01", "2026-08-01"]
                .into_iter()
                .collect();
        let dictionary_batch = RecordBatch::try_new(
            Arc::clone(&dictionary_schema),
            vec![
                Arc::new(days),
                Arc::new(arrow::array::Int64Array::from(vec![1, 2, 3, 4])),
            ],
        )
        .unwrap();
        let output = File::create(&dictionary_file.0).unwrap();
        let mut writer = ArrowWriter::try_new(output, dictionary_schema, None).unwrap();
        writer.write(&dictionary_batch).unwrap();
        writer.close().unwrap();
        let mut reader = ParquetReader::new(&dictionary_file.0)
            .with_columns(vec!["amount".to_owned()])
            .with_predicate(compare(
                "day",
                CompareOp::Eq,
                ScalarValue::Utf8("2026-08-01".into()),
            ))
            .read()
            .unwrap();
        let amounts: Vec<i64> = reader
            .by_ref()
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<arrow::array::Int64Array>()
                    .unwrap()
                    .values()
                    .to_vec()
            })
            .collect();
        assert_eq!(amounts, vec![2, 4]);

        // An OR whose sides are both evaluable is one stage; one with a
        // side the storage layer cannot evaluate is left to the executor.
        let disjunction = StoragePredicate::Or(vec![
            compare("amount", CompareOp::Eq, ScalarValue::Int64(1)),
            compare("amount", CompareOp::Eq, ScalarValue::Int64(6)),
        ]);
        assert_eq!(
            RowFilterPlan::new(&disjunction, &arrow_schema)
                .unwrap()
                .columns(),
            vec![1]
        );
        let half_opaque = StoragePredicate::Or(vec![
            compare("amount", CompareOp::Eq, ScalarValue::Int64(1)),
            StoragePredicate::IsNull {
                column: "elsewhere".into(),
            },
        ]);
        assert!(RowFilterPlan::new(&half_opaque, &arrow_schema).is_none());
        let labels_of = |predicate: StoragePredicate| -> Vec<String> {
            ParquetReader::new(&file.0)
                .with_columns(vec!["label".to_owned()])
                .with_predicate(predicate)
                .read()
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap()
                .iter()
                .flat_map(|batch| {
                    batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .iter()
                        .map(|value| value.unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        assert_eq!(labels_of(disjunction), vec!["a", "f"]);
        // LIKE, NOT LIKE and IN run inside the decoder too.
        assert_eq!(
            labels_of(StoragePredicate::Like {
                column: "day".into(),
                pattern: "2026-08%".into(),
                negated: false,
                case_insensitive: false,
            }),
            vec!["d", "e"]
        );
        assert_eq!(
            labels_of(StoragePredicate::Like {
                column: "day".into(),
                pattern: "%-0_".into(),
                negated: true,
                case_insensitive: false,
            }),
            Vec::<String>::new()
        );
        assert_eq!(
            labels_of(StoragePredicate::In {
                column: "amount".into(),
                values: vec![ScalarValue::Int64(2), ScalarValue::Int64(5)],
            }),
            vec!["b", "e"]
        );
    }

    #[test]
    fn batch_predicate_compares_dictionaries_through_their_values() {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "region",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
            Field::new("amount", DataType::Int64, false),
        ]));
        let regions = arrow::array::DictionaryArray::<arrow::datatypes::Int32Type>::new(
            arrow::array::Int32Array::from(vec![Some(1), Some(0), None, Some(1), Some(2)]),
            Arc::new(StringArray::from(vec!["Asia", "Europe", "Africa"])),
        );
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(regions),
                Arc::new(arrow::array::Int64Array::from(vec![1, 2, 3, 4, 5])),
            ],
        )
        .unwrap();
        let predicate = StoragePredicate::And(vec![
            compare("region", CompareOp::Eq, ScalarValue::Utf8("Europe".into())),
            compare("amount", CompareOp::Lt, ScalarValue::Int64(5)),
        ]);
        let filter = crate::scan_predicate::BatchPredicate::new(&schema, &predicate).unwrap();
        let kept = filter.apply(batch).unwrap();
        let amounts = kept
            .column(1)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap()
            .values()
            .to_vec();
        assert_eq!(amounts, vec![1, 4]); // the null key never matches
    }

    /// Every shape the storage layer evaluates comes back exact from the
    /// local reader: the row groups statistics keep, then the decoder's row
    /// filter within them (the fixture: ids 0..6, label null at id 2, row
    /// groups of three).
    #[test]
    fn supports_boolean_composition_and_null_counts() {
        let file = fixture();
        let range = StoragePredicate::And(vec![
            compare("id", CompareOp::Ge, ScalarValue::Int64(3)),
            compare("id", CompareOp::Lt, ScalarValue::Int64(6)),
        ]);
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(range)),
            ROW_GROUP_SIZE
        );

        let nulls = StoragePredicate::IsNull {
            column: "label".to_owned(),
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(nulls)),
            1
        );

        let non_nulls = StoragePredicate::IsNotNull {
            column: "label".to_owned(),
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(non_nulls)),
            5
        );

        let disjunction = StoragePredicate::Or(vec![
            compare("id", CompareOp::Eq, ScalarValue::Int64(1)),
            compare("id", CompareOp::Eq, ScalarValue::Int64(4)),
        ]);
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(disjunction)),
            2
        );
    }

    #[test]
    fn supports_in_and_not_equal_pruning() {
        let file = fixture();
        let values = StoragePredicate::In {
            column: "id".to_owned(),
            values: vec![ScalarValue::Int64(4), ScalarValue::Int64(99)],
        };
        assert_eq!(
            row_count(&ParquetReader::new(&file.0).with_predicate(values)),
            1
        );

        assert!(!bounds_can_match(
            Some(ScalarValue::Int64(4)),
            Some(ScalarValue::Int64(4)),
            CompareOp::Ne,
            &ScalarValue::Int64(4),
        ));
    }

    #[test]
    fn reports_measured_scan_metrics() {
        let file = fixture();
        let mut source = ParquetReader::new(&file.0)
            .with_batch_size(2)
            .with_columns(vec!["id".to_owned()])
            .with_predicate(compare("id", CompareOp::Eq, ScalarValue::Int64(4)))
            .read()
            .expect("reader must open");
        let metrics = source.metrics();

        let planned = metrics.snapshot();
        assert_eq!(planned.files_considered, 1);
        assert_eq!(planned.files_opened, 1);
        assert_eq!(planned.row_groups_considered, 2);
        assert_eq!(planned.row_groups_selected, 1);
        assert_eq!(planned.row_groups_pruned(), 1);
        assert_eq!(planned.rows_selected, ROW_GROUP_SIZE as u64);
        assert!(planned.compressed_bytes_selected > 0);
        assert_eq!(planned.rows_emitted, 0);

        while source
            .next()
            .transpose()
            .expect("batch must decode")
            .is_some()
        {}
        let completed = metrics.snapshot();
        // One row group selected, its rows filtered exactly to the match.
        assert_eq!(completed.rows_emitted, 1);
        assert_eq!(completed.batches_emitted, 1);
        assert!(completed.rows_per_second().is_finite());
        assert!(completed.compressed_bytes_per_second().is_finite());
    }

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(path: &Path, schema: &SchemaRef, ids: Vec<i32>, row_group_size: usize) {
        let labels = ids
            .iter()
            .map(|id| Some(format!("l{id}")))
            .collect::<Vec<_>>();
        let mut columns: Vec<ArrayRef> = vec![Arc::new(Int32Array::from(ids))];
        if schema.fields().len() > 1 {
            columns.push(Arc::new(StringArray::from(labels)));
        }
        let batch = RecordBatch::try_new(Arc::clone(schema), columns).unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let mut writer = ArrowWriter::try_new(
            File::create(path).unwrap(),
            Arc::clone(schema),
            Some(properties),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    /// A directory table: two small files, one large partitioned file, and
    /// the hidden and marker files a writer leaves behind.
    fn directory_fixture() -> (TestDirectory, SchemaRef) {
        let id = NEXT_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let directory = TestDirectory(std::env::temp_dir().join(format!(
            "kaveon-storage-directory-{}-{id}",
            std::process::id()
        )));
        std::fs::create_dir_all(directory.0.join("year=2026")).unwrap();
        std::fs::create_dir_all(directory.0.join("_delta_log")).unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("label", DataType::Utf8, true),
        ]));
        write_file(&directory.0.join("b.parquet"), &schema, vec![3, 4], 1);
        write_file(&directory.0.join("a.parquet"), &schema, vec![1, 2], 1);
        write_file(
            &directory.0.join("year=2026").join("c.PARQUET"),
            &schema,
            (5..=104).collect(),
            10,
        );
        write_file(&directory.0.join(".hidden.parquet"), &schema, vec![99], 1);
        std::fs::write(directory.0.join("_SUCCESS"), b"").unwrap();
        std::fs::write(directory.0.join("_delta_log").join("0.json"), b"{}").unwrap();
        std::fs::write(directory.0.join("empty-marker"), b"").unwrap();
        (directory, schema)
    }

    fn ids(iterator: ParquetBatchIterator) -> Vec<i32> {
        let mut ids = Vec::new();
        for batch in iterator {
            let batch = batch.unwrap();
            let column = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            ids.extend(column.values().iter().copied());
        }
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_directory_is_a_table_read_once_across_partitions() {
        let (directory, schema) = directory_fixture();
        let metadata = ParquetReader::new(&directory.0).metadata().unwrap();
        assert_eq!(metadata.row_count, 104);
        assert_eq!(metadata.row_group_count, 14);
        assert_eq!(metadata.schema, schema);

        let mut seen = Vec::new();
        let mut considered = 0;
        for index in 0..2 {
            let metrics = ScanMetrics::default();
            let iterator = ParquetReader::new(&directory.0)
                .with_columns(vec!["label".into(), "id".into()])
                .with_partition(ScanPartition::new(index, 2).unwrap())
                .with_metrics(metrics.clone())
                .read()
                .unwrap();
            assert_eq!(iterator.schema().field(0).name(), "label");
            for batch in iterator {
                let batch = batch.unwrap();
                assert_eq!(batch.schema().field(1).name(), "id");
                let column = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int32Array>()
                    .unwrap();
                seen.extend(column.values().iter().copied());
            }
            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.files_opened, snapshot.files_considered);
            considered += snapshot.files_considered;
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=104).collect::<Vec<_>>());
        // The large file is split by row group between the two partitions,
        // the two small files are read whole by one each.
        assert_eq!(considered, 4);

        let metrics = ScanMetrics::default();
        let pruned = ParquetReader::new(&directory.0)
            .with_predicate(compare("id", CompareOp::Ge, ScalarValue::Int64(100)))
            .with_metrics(metrics.clone())
            .read()
            .unwrap();
        assert_eq!(ids(pruned), [100, 101, 102, 103, 104]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_considered, 3);
        assert_eq!(snapshot.row_groups_considered, 14);
        assert_eq!(snapshot.row_groups_selected, 1);
    }

    #[test]
    fn a_directory_file_with_another_schema_is_named() {
        let (directory, _) = directory_fixture();
        let narrow = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        write_file(&directory.0.join("d.parquet"), &narrow, vec![7], 1);
        let failure = ParquetReader::new(&directory.0)
            .metadata()
            .expect_err("a second schema is an error")
            .to_string();
        assert!(failure.contains("d.parquet"), "{failure}");
        assert!(failure.contains("a.parquet"), "{failure}");
        let failure = ParquetReader::new(&directory.0)
            .read()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .expect_err("a second schema is an error when read")
            .to_string();
        assert!(failure.contains("d.parquet"), "{failure}");

        std::fs::write(directory.0.join("notes.txt"), b"x").unwrap();
        let failure = ParquetReader::new(&directory.0)
            .read()
            .err()
            .expect("a foreign file is an error")
            .to_string();
        assert!(failure.contains("notes.txt"), "{failure}");
    }
}
