//! Directory Parquet tables.
//!
//! A catalog location that names a directory of Parquet files rather than one
//! object — the Hive/Spark layout, what Trino writes — is a table. The
//! directory is listed once per scan, the listing is sorted so every reader
//! of it sees the same files in the same order, the files are spread over the
//! scan partitions by size, and every file is read through the per-object
//! reader with its own identity-pinned footer cache. The schema is the first
//! file's; every other file is checked against it and a difference is an
//! error naming the file, never a cast.
//!
//! Which objects are data files follows Hive and Spark: an object whose name,
//! or any directory below the root, begins with `_` or `.` is hidden
//! (`_SUCCESS`, `_delta_log/…`, `.part-….crc`); a zero-byte object holds no
//! rows and is skipped; every other object is data when it carries the
//! `.parquet` extension or no extension at all (Trino's layout). An object
//! with any other extension is an error naming it, so a stray file cannot be
//! silently read as data or silently dropped from the table.
//!
//! **Partition columns.** Every directory between the root and a data file
//! is a `key=value` segment (Hive's layout): the keys are columns of the
//! table, appended after the file columns, and each file's rows carry the
//! values of its path. The value is Hive-decoded (`%XX` escapes) and
//! `__HIVE_DEFAULT_PARTITION__` is NULL. Every file must carry the same keys
//! in the same order — a file at another depth or under other keys is an
//! error naming both files — and a key that is also a column inside the
//! files is an error naming the file. A key's type is inferred from its
//! values: `bigint` when every non-null value is a canonical integer,
//! `date` when every one is `YYYY-MM-DD`, else `varchar`; the table's
//! catalog definition can declare the type instead (`with_catalog_schema`),
//! and a value that does not read as the declared type is an error naming
//! the file. Text partitions are handed out dictionary-encoded (one value,
//! `Int32` keys), integers and dates as plain arrays.
//!
//! **Pruning.** A scan predicate is folded over each file's partition values
//! before any file is opened: a file whose values make the predicate false
//! or NULL is pruned — never listed as considered, counted under
//! `files_pruned_by_partition` — and what the fold leaves open on the file
//! columns is the predicate the file's reader runs. Comparisons, `IN`,
//! `IS [NOT] NULL`, `LIKE`, and `AND`/`OR`/`NOT` over them fold; a term the
//! path cannot decide (a literal of another type, a column not in the path)
//! keeps the file and weakens the residual, and `NOT` is folded only over
//! an exactly-folded operand, since the negation of a weaker predicate is
//! not implied. Files are spread over the scan partitions after pruning.
//!
//! The listing is not carried to the workers of a distributed query: the
//! executable fragment names the location and every task lists it under the
//! same deterministic rule (the fragment wire format is unchanged). The
//! coordinator pins its own listing per query through the planning source
//! pins, the way it pins Delta versions.

use std::sync::{Arc, mpsc};

use arrow::{
    array::{Array, ArrayRef, Date32Array, DictionaryArray, Int32Array, Int64Array, StringArray},
    compute::kernels::comparison::{ilike, like, nilike, nlike},
    datatypes::{DataType, Field, Int32Type, Schema, SchemaRef},
    record_batch::RecordBatch,
};
use futures::{StreamExt, TryStreamExt, stream};
use kaveon_core::{
    BatchSource, CompareOp, PartitionColumn, Result, ScalarValue, StoragePredicate,
    predicate::date_literal_days,
};
use object_store::{ObjectStore, path::Path};

use crate::{
    FooterProfile, ParquetFileMetadata, ScanMetrics, ScanPartition,
    adls_reader::{AdlsParquetReader, OpenError, adls_store, scan_parallelism},
    object_reader::{ObjectLocation, error, storage_error},
    parquet_reader::projection_indices,
};

const DEFAULT_BATCH_SIZE: usize = 8_192;
const METADATA_READ_CONCURRENCY: usize = 16;
/// A partition may carry this fraction more than its fair share of bytes in
/// whole files before the largest whole file is split by row group instead:
/// one quarter. Below it, whole files keep their locality (one footer, one
/// full-object cache entry, sequential ranges on one reader).
const IMBALANCE_TOLERANCE_NUMERATOR: u128 = 5;
const IMBALANCE_TOLERANCE_DENOMINATOR: u128 = 4;
/// Hive's name for the NULL partition.
pub const HIVE_DEFAULT_PARTITION: &str = "__HIVE_DEFAULT_PARTITION__";

/// One data file of a directory table, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryFile {
    /// Store-relative path of the object.
    pub path: Path,
    pub size: u64,
    pub e_tag: Option<String>,
    pub version: Option<String>,
    /// Last modification, nanoseconds since the epoch, for identity only.
    pub modified_nanos: i64,
    /// The `key=value` segments between the root and the file, decoded,
    /// one per partition column of the listing; `None` is the NULL
    /// partition.
    pub partition_values: Vec<Option<String>>,
}

impl DirectoryFile {
    /// The immutable identity the store gives the object: its ETag or version.
    pub fn identity(&self) -> Option<&str> {
        self.e_tag.as_deref().or(self.version.as_deref())
    }
}

/// The data files under a directory root, sorted by path, and the partition
/// columns their paths carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryListing {
    pub root: Path,
    pub files: Vec<DirectoryFile>,
    /// The `key=value` keys every file carries, in path order, each with the
    /// type inferred from its values. Empty for a flat directory.
    pub partitions: Vec<PartitionColumn>,
}

impl DirectoryListing {
    pub fn total_bytes(&self) -> u64 {
        self.files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size))
    }

    pub fn sizes(&self) -> Vec<u64> {
        self.files.iter().map(|file| file.size).collect()
    }

    /// One line per file — path, size, identity — for digests.
    pub fn identity_lines(&self) -> String {
        let mut lines = String::new();
        for file in &self.files {
            lines.push_str(file.path.as_ref());
            lines.push('\t');
            lines.push_str(&file.size.to_string());
            lines.push('\t');
            lines.push_str(file.identity().unwrap_or_default());
            lines.push('\n');
        }
        lines
    }

    /// The listing reduced to the files at `indices` (ascending, as listed);
    /// the partition columns stay.
    pub fn retain(&self, indices: &[usize]) -> DirectoryListing {
        DirectoryListing {
            root: self.root.clone(),
            files: indices
                .iter()
                .map(|index| self.files[*index].clone())
                .collect(),
            partitions: self.partitions.clone(),
        }
    }

    /// The listing without the files `predicate` rules out by their path
    /// values, the partition types taken from `catalog_schema` where it
    /// names them; what the planner pins for a query whose scan predicate
    /// is known.
    pub fn pruned_by(
        &self,
        catalog_schema: Option<&SchemaRef>,
        predicate: &StoragePredicate,
    ) -> Result<DirectoryListing> {
        let layout = PartitionLayout::of(self, catalog_schema)?;
        let pruned = prune_files(&layout, Some(predicate));
        Ok(self.retain(
            &pruned
                .kept
                .iter()
                .map(|file| file.index)
                .collect::<Vec<_>>(),
        ))
    }
}

/// Whether a listed object is data, hidden, or something a table must not
/// silently contain.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileVerdict {
    Data,
    Hidden,
    Foreign,
}

/// Classify an object by its path relative to the directory root.
pub(crate) fn file_verdict<'a>(relative: impl Iterator<Item = &'a str>) -> FileVerdict {
    let mut name = "";
    for segment in relative {
        if segment.starts_with('_') || segment.starts_with('.') {
            return FileVerdict::Hidden;
        }
        name = segment;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        None => FileVerdict::Data,
        Some(extension) if extension.eq_ignore_ascii_case("parquet") => FileVerdict::Data,
        Some(_) => FileVerdict::Foreign,
    }
}

/// Hive's path escaping undone: `%XX` is the byte `XX`; a `%` that no two
/// hex digits follow stands for itself, as Hive reads it.
fn hive_decode(segment: &str) -> String {
    if !segment.contains('%') {
        return segment.to_owned();
    }
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = (bytes[index] == b'%' && index + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[index + 1..index + 3]).ok())
            .flatten()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match hex {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// A decoded `key=value` segment: the key and its value, `None` for the
/// NULL partition.
type PartitionSegment = (String, Option<String>);

/// A `key=value` directory segment, decoded; `None` for a segment that is
/// not one.
fn partition_segment(segment: &str) -> Option<PartitionSegment> {
    let (key, value) = segment.split_once('=')?;
    if key.is_empty() {
        return None;
    }
    let value = hive_decode(value);
    let value = (value != HIVE_DEFAULT_PARTITION).then_some(value);
    Some((hive_decode(key), value))
}

/// Whether `value` is an integer in its canonical spelling — the only
/// spelling that reads back as the same text — so `007` and `+1` stay text.
fn canonical_integer(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
    {
        return None;
    }
    value.parse().ok()
}

/// `YYYY-MM-DD`, exactly, as days since the epoch.
fn canonical_date(value: &str) -> Option<i32> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return None;
    }
    date_literal_days(value).and_then(|days| i32::try_from(days).ok())
}

/// The type a key's values read as: integer when every non-null value is a
/// canonical integer, date when every one is `YYYY-MM-DD`, else text.
fn infer_partition_type<'a>(values: impl Iterator<Item = &'a str> + Clone) -> DataType {
    let mut any = false;
    let mut integers = true;
    let mut dates = true;
    for value in values {
        any = true;
        integers = integers && canonical_integer(value).is_some();
        dates = dates && canonical_date(value).is_some();
        if !integers && !dates {
            break;
        }
    }
    if any && integers {
        DataType::Int64
    } else if any && dates {
        DataType::Date32
    } else {
        DataType::Utf8
    }
}

/// The partition keys of every listed file, checked to agree, with their
/// types inferred.
fn partition_columns(
    root: &Path,
    files: &[(Path, Vec<PartitionSegment>)],
) -> Result<Vec<PartitionColumn>> {
    let Some((first_path, first)) = files.first() else {
        return Ok(Vec::new());
    };
    for (path, keys) in files.iter().skip(1) {
        let same = keys.len() == first.len()
            && keys
                .iter()
                .zip(first)
                .all(|((key, _), (expected, _))| key == expected);
        if !same {
            let describe = |keys: &[PartitionSegment]| {
                if keys.is_empty() {
                    "no partition keys".to_owned()
                } else {
                    keys.iter()
                        .map(|(key, _)| key.as_str())
                        .collect::<Vec<_>>()
                        .join("/")
                }
            };
            return Err(error(format!(
                "Parquet directory '{root}': '{path}' is under {} where '{first_path}' is under \
                 {}; every file of a partitioned directory carries the same keys in the same \
                 order",
                describe(keys),
                describe(first)
            )));
        }
    }
    first
        .iter()
        .enumerate()
        .map(|(position, (key, _))| {
            let data_type = infer_partition_type(
                files
                    .iter()
                    .filter_map(move |(_, keys)| keys[position].1.as_deref()),
            );
            PartitionColumn::new(key.clone(), data_type)
        })
        .collect()
}

/// List the data files under `root`. The store's listing is recursive; the
/// `key=value` directories of a partitioned layout become the listing's
/// partition columns, every file checked to carry the same keys.
pub async fn list_parquet_directory(
    store: &dyn ObjectStore,
    root: &Path,
) -> Result<DirectoryListing> {
    let prefix = (!root.as_ref().is_empty()).then_some(root);
    let mut objects = store.list(prefix);
    let mut files: Vec<(DirectoryFile, Vec<PartitionSegment>)> = Vec::new();
    while let Some(object) = objects.try_next().await.map_err(storage_error)? {
        let Some(relative) = object.location.prefix_match(root) else {
            continue;
        };
        let relative = relative
            .map(|segment| segment.as_ref().to_owned())
            .collect::<Vec<_>>();
        match file_verdict(relative.iter().map(String::as_str)) {
            FileVerdict::Hidden => continue,
            FileVerdict::Foreign => {
                return Err(error(format!(
                    "Parquet directory '{root}' contains '{}', which is neither a .parquet file nor \n                     an extension-less data file; remove it or hide it with a leading '_' or '.'",
                    object.location
                )));
            }
            FileVerdict::Data => {}
        }
        if object.size == 0 {
            continue;
        }
        let Some((_, directories)) = relative.split_last() else {
            continue;
        };
        let mut segments = Vec::with_capacity(directories.len());
        for directory in directories {
            let Some(segment) = partition_segment(directory) else {
                return Err(error(format!(
                    "Parquet directory '{root}': '{}' lies under directory '{directory}', which \n                     is not a key=value partition segment; a partitioned directory holds its \n                     files under key=value directories only",
                    object.location
                )));
            };
            segments.push(segment);
        }
        let file = DirectoryFile {
            partition_values: segments.iter().map(|(_, value)| value.clone()).collect(),
            path: object.location,
            size: u64::try_from(object.size).map_err(storage_error)?,
            e_tag: object.e_tag,
            version: object.version,
            modified_nanos: object
                .last_modified
                .timestamp_nanos_opt()
                .unwrap_or_default(),
        };
        files.push((file, segments));
    }
    files.sort_by(|(left, _), (right, _)| left.path.as_ref().cmp(right.path.as_ref()));
    let keys = files
        .iter()
        .map(|(file, segments)| (file.path.clone(), segments.clone()))
        .collect::<Vec<_>>();
    let partitions = partition_columns(root, &keys)?;
    Ok(DirectoryListing {
        root: root.clone(),
        files: files.into_iter().map(|(file, _)| file).collect(),
        partitions,
    })
}

/// A partition value as a column of its type carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartitionValue {
    Null,
    Int64(i64),
    /// Days since the epoch.
    Date32(i32),
    Utf8(String),
}

impl PartitionValue {
    fn parse(
        raw: Option<&str>,
        column: &PartitionColumn,
        root: &Path,
        file: &Path,
    ) -> Result<Self> {
        let Some(raw) = raw else {
            return Ok(Self::Null);
        };
        let unreadable = |kind: &str| {
            error(format!(
                "Parquet directory '{root}': partition value '{raw}' of '{}' in '{file}' is not \
                 a {kind}; declare the column as varchar or fix the path",
                column.name()
            ))
        };
        match column.data_type() {
            DataType::Int64 => canonical_integer(raw)
                .map(Self::Int64)
                .ok_or_else(|| unreadable("bigint")),
            DataType::Date32 => canonical_date(raw)
                .map(Self::Date32)
                .ok_or_else(|| unreadable("date")),
            _ => Ok(Self::Utf8(raw.to_owned())),
        }
    }

    /// `rows` copies of the value as an array of `data_type`: text as a
    /// one-entry dictionary, numbers and dates plain.
    fn array(&self, data_type: &DataType, rows: usize) -> Result<ArrayRef> {
        Ok(match (self, data_type) {
            (Self::Null, _) => arrow::array::new_null_array(data_type, rows),
            (Self::Int64(value), DataType::Int64) => Arc::new(Int64Array::from(vec![*value; rows])),
            (Self::Date32(days), DataType::Date32) => {
                Arc::new(Date32Array::from(vec![*days; rows]))
            }
            (Self::Utf8(text), DataType::Dictionary(_, _)) => {
                let values: ArrayRef = Arc::new(StringArray::from(vec![text.as_str()]));
                Arc::new(
                    DictionaryArray::<Int32Type>::try_new(Int32Array::from(vec![0; rows]), values)
                        .map_err(storage_error)?,
                )
            }
            (value, other) => {
                return Err(error(format!(
                    "partition value {value:?} cannot be presented as {other}"
                )));
            }
        })
    }
}

/// The Arrow type a partition column is presented as.
pub fn partition_field(column: &PartitionColumn) -> Field {
    let data_type = match column.data_type() {
        DataType::Utf8 => DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        other => other.clone(),
    };
    Field::new(column.name(), data_type, true)
}

/// The partition columns a scan produces — the listing's, typed by the
/// catalog where it names them — and each file's values under those types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionLayout {
    pub columns: Vec<PartitionColumn>,
    /// One vector per listed file, in listing order.
    pub values: Vec<Vec<PartitionValue>>,
    pub sizes: Vec<u64>,
}

impl PartitionLayout {
    /// The listing's partition columns with the catalog's types where the
    /// catalog names them (a declared type stands; a declared type no path
    /// value can carry is an error), every file's values parsed under them.
    pub fn of(listing: &DirectoryListing, catalog_schema: Option<&SchemaRef>) -> Result<Self> {
        let columns = listing
            .partitions
            .iter()
            .map(|inferred| {
                match catalog_schema.and_then(|schema| schema.field_with_name(inferred.name()).ok())
                {
                    Some(field) => PartitionColumn::new(inferred.name(), field.data_type().clone())
                        .map_err(|failure| {
                            error(format!("Parquet directory '{}': {failure}", listing.root))
                        }),
                    None => Ok(inferred.clone()),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let values = listing
            .files
            .iter()
            .map(|file| {
                columns
                    .iter()
                    .zip(&file.partition_values)
                    .map(|(column, raw)| {
                        PartitionValue::parse(raw.as_deref(), column, &listing.root, &file.path)
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            columns,
            values,
            sizes: listing.sizes(),
        })
    }

    /// The fields appended to the file schema.
    pub fn fields(&self) -> Vec<Field> {
        self.columns.iter().map(partition_field).collect()
    }
}

/// SQL's three truth values, for a predicate folded over constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlBool {
    True,
    False,
    Null,
}

impl SqlBool {
    fn of(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Null => Self::Null,
        }
    }
}

/// A predicate folded over one file's partition values.
#[derive(Clone, Debug, PartialEq)]
enum Folded {
    /// Decided for every row of the file.
    Known(SqlBool),
    /// Undecidable from the path: the file is kept and the term dropped.
    Unknown,
    /// Depends on the file's own columns. `exact` when nothing was dropped
    /// on the way, so the predicate may be negated.
    Open {
        predicate: StoragePredicate,
        exact: bool,
    },
}

fn compare_values(value: &PartitionValue, literal: &ScalarValue) -> Option<std::cmp::Ordering> {
    match (value, literal) {
        (PartitionValue::Int64(left), ScalarValue::Int64(right)) => Some(left.cmp(right)),
        (PartitionValue::Date32(left), ScalarValue::Int64(right)) => {
            Some(i64::from(*left).cmp(right))
        }
        (PartitionValue::Date32(left), ScalarValue::Utf8(text)) => {
            date_literal_days(text).map(|days| i64::from(*left).cmp(&days))
        }
        (PartitionValue::Utf8(left), ScalarValue::Utf8(right)) => Some(left.as_str().cmp(right)),
        _ => None,
    }
}

fn fold_compare(value: &PartitionValue, op: CompareOp, literal: &ScalarValue) -> Folded {
    if matches!(value, PartitionValue::Null) {
        return Folded::Known(SqlBool::Null);
    }
    let Some(ordering) = compare_values(value, literal) else {
        return Folded::Unknown;
    };
    let holds = match op {
        CompareOp::Eq => ordering.is_eq(),
        CompareOp::Ne => ordering.is_ne(),
        CompareOp::Lt => ordering.is_lt(),
        CompareOp::Le => ordering.is_le(),
        CompareOp::Gt => ordering.is_gt(),
        CompareOp::Ge => ordering.is_ge(),
    };
    Folded::Known(SqlBool::of(holds))
}

fn fold_in(value: &PartitionValue, literals: &[ScalarValue]) -> Folded {
    if matches!(value, PartitionValue::Null) {
        return Folded::Known(SqlBool::Null);
    }
    let mut saw_null = false;
    for literal in literals {
        if matches!(literal, ScalarValue::Null) {
            saw_null = true;
            continue;
        }
        match compare_values(value, literal) {
            Some(ordering) if ordering.is_eq() => return Folded::Known(SqlBool::True),
            Some(_) => {}
            None => return Folded::Unknown,
        }
    }
    Folded::Known(if saw_null {
        SqlBool::Null
    } else {
        SqlBool::False
    })
}

fn fold_like(
    value: &PartitionValue,
    pattern: &str,
    negated: bool,
    case_insensitive: bool,
) -> Folded {
    let text = match value {
        PartitionValue::Null => return Folded::Known(SqlBool::Null),
        PartitionValue::Utf8(text) => text.as_str(),
        _ => return Folded::Unknown,
    };
    let values = StringArray::from(vec![text]);
    let pattern = arrow::array::Scalar::new(StringArray::from(vec![pattern]));
    let matched = match (negated, case_insensitive) {
        (false, false) => like(&values, &pattern),
        (true, false) => nlike(&values, &pattern),
        (false, true) => ilike(&values, &pattern),
        (true, true) => nilike(&values, &pattern),
    };
    match matched {
        Ok(matched) if matched.len() == 1 && matched.is_valid(0) => {
            Folded::Known(SqlBool::of(matched.value(0)))
        }
        _ => Folded::Unknown,
    }
}

fn fold_children(
    children: &[StoragePredicate],
    layout: &PartitionLayout,
    values: &[PartitionValue],
    conjunction: bool,
) -> Folded {
    let (absorbing, neutral) = if conjunction {
        (SqlBool::False, SqlBool::True)
    } else {
        (SqlBool::True, SqlBool::False)
    };
    let mut open = Vec::new();
    let mut exact = true;
    let mut unknown = false;
    let mut saw_null = false;
    for child in children {
        match fold_predicate(child, layout, values) {
            Folded::Known(value) if value == absorbing => return Folded::Known(absorbing),
            Folded::Known(value) if value == neutral => {}
            Folded::Known(_) => saw_null = true,
            Folded::Unknown => {
                unknown = true;
                exact = false;
            }
            Folded::Open {
                predicate,
                exact: child_exact,
            } => {
                exact = exact && child_exact;
                open.push(predicate);
            }
        }
    }
    if conjunction && saw_null {
        // AND with a NULL term is NULL or false: no row passes.
        return Folded::Known(SqlBool::Null);
    }
    if open.is_empty() {
        return if unknown {
            Folded::Unknown
        } else if saw_null {
            Folded::Known(SqlBool::Null)
        } else {
            Folded::Known(neutral)
        };
    }
    if !conjunction && (unknown || saw_null) {
        // OR with an undecided or NULL term: a residual on the file columns
        // alone would reject rows the dropped term admits.
        return Folded::Unknown;
    }
    let predicate = if open.len() == 1 {
        open.pop().expect("one predicate")
    } else if conjunction {
        StoragePredicate::And(open)
    } else {
        StoragePredicate::Or(open)
    };
    Folded::Open { predicate, exact }
}

/// Fold `predicate` over one file's partition values.
fn fold_predicate(
    predicate: &StoragePredicate,
    layout: &PartitionLayout,
    values: &[PartitionValue],
) -> Folded {
    let partition = |column: &str| {
        layout
            .columns
            .iter()
            .position(|candidate| candidate.name() == column)
            .map(|index| &values[index])
    };
    let open = || Folded::Open {
        predicate: predicate.clone(),
        exact: true,
    };
    match predicate {
        StoragePredicate::Compare { column, op, value } => match partition(column) {
            Some(constant) => fold_compare(constant, *op, value),
            None => open(),
        },
        StoragePredicate::IsNull { column } => match partition(column) {
            Some(constant) => Folded::Known(SqlBool::of(matches!(constant, PartitionValue::Null))),
            None => open(),
        },
        StoragePredicate::IsNotNull { column } => match partition(column) {
            Some(constant) => Folded::Known(SqlBool::of(!matches!(constant, PartitionValue::Null))),
            None => open(),
        },
        StoragePredicate::In {
            column,
            values: literals,
        } => match partition(column) {
            Some(constant) => fold_in(constant, literals),
            None => open(),
        },
        StoragePredicate::Like {
            column,
            pattern,
            negated,
            case_insensitive,
        } => match partition(column) {
            Some(constant) => fold_like(constant, pattern, *negated, *case_insensitive),
            None => open(),
        },
        StoragePredicate::And(children) => fold_children(children, layout, values, true),
        StoragePredicate::Or(children) => fold_children(children, layout, values, false),
        StoragePredicate::Not(inner) => match fold_predicate(inner, layout, values) {
            Folded::Known(value) => Folded::Known(value.not()),
            Folded::Open {
                predicate,
                exact: true,
            } => Folded::Open {
                predicate: StoragePredicate::Not(Box::new(predicate)),
                exact: true,
            },
            Folded::Open { exact: false, .. } | Folded::Unknown => Folded::Unknown,
        },
    }
}

/// A file a scan opens after pruning.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptFile {
    /// Index into the listing.
    pub index: usize,
    /// What the predicate leaves to the file's own columns; `None` when it
    /// is satisfied by the path alone or there was no predicate.
    pub residual: Option<StoragePredicate>,
}

/// The outcome of pruning a listing under a predicate.
#[derive(Clone, Debug, PartialEq)]
pub struct PrunedFiles {
    pub kept: Vec<KeptFile>,
    /// Listing indices of the files the predicate ruled out.
    pub pruned: Vec<usize>,
}

impl PrunedFiles {
    /// How many pruned files this scan partition reports: the pruned files
    /// dealt round-robin over the partitions, so the tasks of a query sum
    /// to the total.
    pub fn pruned_share(&self, partition: Option<ScanPartition>) -> u64 {
        match partition {
            Some(partition) => (0..self.pruned.len())
                .filter(|ordinal| partition.contains(*ordinal))
                .count() as u64,
            None => self.pruned.len() as u64,
        }
    }
}

/// Fold `predicate` over every file's partition values: which files survive
/// and the residual predicate each carries. Without partition columns or a
/// predicate every file survives with the predicate whole.
pub fn prune_files(layout: &PartitionLayout, predicate: Option<&StoragePredicate>) -> PrunedFiles {
    let mut kept = Vec::with_capacity(layout.values.len());
    let mut pruned = Vec::new();
    for (index, values) in layout.values.iter().enumerate() {
        let residual = match predicate {
            None => None,
            Some(_) if layout.columns.is_empty() => predicate.cloned(),
            Some(predicate) => match fold_predicate(predicate, layout, values) {
                Folded::Known(SqlBool::True) | Folded::Unknown => None,
                Folded::Known(SqlBool::False | SqlBool::Null) => {
                    pruned.push(index);
                    continue;
                }
                Folded::Open { predicate, .. } => Some(predicate),
            },
        };
        kept.push(KeptFile { index, residual });
    }
    PrunedFiles { kept, pruned }
}

/// Which files of a listing one scan partition reads, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAssignment {
    /// Files this partition reads whole, as indices into the listing.
    pub whole: Vec<usize>,
    /// Files every partition shares by row group: the partition applies
    /// inside each of them, exactly as it does to a single-file table.
    pub split: Vec<usize>,
}

impl FileAssignment {
    /// The files this partition opens, in listing order, with whether the
    /// partition applies inside the file.
    pub fn files(&self) -> Vec<(usize, bool)> {
        let mut files = self
            .whole
            .iter()
            .map(|index| (*index, false))
            .chain(self.split.iter().map(|index| (*index, true)))
            .collect::<Vec<_>>();
        files.sort_unstable();
        files
    }

    pub fn len(&self) -> usize {
        self.whole.len() + self.split.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Spread files over the scan partitions by size. Whole files go to the
/// partition with the fewest bytes so far, largest first (ties by listing
/// order). If the heaviest partition would then carry more than a quarter
/// over its fair share, the largest whole file is split by row group across
/// every partition instead, and the rest are placed again; a single-file
/// table therefore splits by row group as it always has. The result is a
/// pure function of the sizes and the partition count, so every task of a
/// query derives the same assignment from the same listing.
pub fn assign_files(sizes: &[u64], partition: ScanPartition) -> FileAssignment {
    if partition.count == 1 {
        return FileAssignment {
            whole: (0..sizes.len()).collect(),
            split: Vec::new(),
        };
    }
    let count = partition.count;
    let total = sizes.iter().map(|size| u128::from(*size)).sum::<u128>();
    let fair_share = total.div_ceil(count as u128);
    let mut whole_order = (0..sizes.len()).collect::<Vec<_>>();
    whole_order.sort_by(|left, right| sizes[*right].cmp(&sizes[*left]).then(left.cmp(right)));
    let mut split = Vec::new();
    let mut split_bytes = 0_u128;
    let mut owner = vec![0_usize; sizes.len()];
    loop {
        let mut loads = vec![0_u128; count];
        for index in &whole_order {
            let lightest = (0..count)
                .min_by_key(|candidate| (loads[*candidate], *candidate))
                .expect("partition count is positive");
            loads[lightest] += u128::from(sizes[*index]);
            owner[*index] = lightest;
        }
        let heaviest =
            loads.iter().copied().max().unwrap_or_default() + split_bytes / count as u128;
        let balanced = heaviest * IMBALANCE_TOLERANCE_DENOMINATOR
            <= fair_share * IMBALANCE_TOLERANCE_NUMERATOR;
        if balanced || whole_order.is_empty() {
            break;
        }
        let largest = whole_order.remove(0);
        split_bytes += u128::from(sizes[largest]);
        split.push(largest);
    }
    let mut whole = whole_order
        .into_iter()
        .filter(|index| owner[*index] == partition.index)
        .collect::<Vec<_>>();
    whole.sort_unstable();
    split.sort_unstable();
    FileAssignment { whole, split }
}

/// The files a scan partition opens after pruning, in listing order, each
/// with its residual predicate and whether the partition applies inside it.
/// `assign_files` runs over the kept files' sizes alone, so the pruned bytes
/// never weigh on the balance.
pub fn assign_kept_files(
    pruned: &PrunedFiles,
    sizes: &[u64],
    partition: Option<ScanPartition>,
) -> Vec<(KeptFile, bool)> {
    let assignment = match partition {
        Some(partition) => assign_files(
            &pruned
                .kept
                .iter()
                .map(|file| sizes[file.index])
                .collect::<Vec<_>>(),
            partition,
        ),
        None => FileAssignment {
            whole: (0..pruned.kept.len()).collect(),
            split: Vec::new(),
        },
    };
    assignment
        .files()
        .into_iter()
        .map(|(position, split)| (pruned.kept[position].clone(), split))
        .collect()
}

/// The schema every file of a directory table must present: the first file's
/// names, order and types. A file may declare a column non-nullable where the
/// first declares it nullable (its arrays fit), never the reverse.
pub(crate) fn check_file_schema(
    root: &str,
    first: &str,
    expected: &SchemaRef,
    file: &str,
    actual: &SchemaRef,
) -> Result<()> {
    if expected.fields().len() != actual.fields().len() {
        return Err(error(format!(
            "Parquet directory '{root}': '{file}' has {} columns where '{first}' has {}; a \
             directory table has one schema",
            actual.fields().len(),
            expected.fields().len()
        )));
    }
    for (position, (expected, actual)) in expected.fields().iter().zip(actual.fields()).enumerate()
    {
        if expected.name() != actual.name() {
            return Err(error(format!(
                "Parquet directory '{root}': '{file}' has column '{}' at position {position} \
                 where '{first}' has '{}'; a directory table has one schema",
                actual.name(),
                expected.name()
            )));
        }
        if expected.data_type() != actual.data_type() {
            return Err(error(format!(
                "Parquet directory '{root}': column '{}' is {} in '{file}' but {} in '{first}'; a \
                 directory table has one schema and no file is cast to another's",
                expected.name(),
                actual.data_type(),
                expected.data_type()
            )));
        }
        if actual.is_nullable() && !expected.is_nullable() {
            return Err(error(format!(
                "Parquet directory '{root}': column '{}' is nullable in '{file}' but not in \
                 '{first}'; a directory table has one schema",
                expected.name()
            )));
        }
    }
    Ok(())
}

/// A partition key that is also a column inside a file would give the
/// column two sources; it is an error naming the file.
pub(crate) fn check_partition_columns_absent(
    root: &str,
    file: &str,
    file_schema: &SchemaRef,
    partitions: &[PartitionColumn],
) -> Result<()> {
    for column in partitions {
        if file_schema.field_with_name(column.name()).is_ok() {
            return Err(error(format!(
                "Parquet directory '{root}': partition column '{}' from the path is also a \
                 column inside '{file}'; a column comes from the path or from the files, not both",
                column.name()
            )));
        }
    }
    Ok(())
}

/// The table schema of a directory: the file schema with the partition
/// columns appended.
pub(crate) fn table_schema(file_schema: &SchemaRef, layout: &PartitionLayout) -> SchemaRef {
    if layout.columns.is_empty() {
        return Arc::clone(file_schema);
    }
    let mut fields = file_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect::<Vec<_>>();
    fields.extend(layout.fields());
    Arc::new(Schema::new_with_metadata(
        fields,
        file_schema.metadata().clone(),
    ))
}

/// Where each advertised column of a directory scan comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnSource {
    /// Position in the batch the file reader hands back.
    File(usize),
    /// Index into the layout's partition columns.
    Partition(usize),
}

/// How a directory scan assembles its batches: which columns the per-file
/// reader is asked for, and how the advertised columns are drawn from the
/// file batch and the file's partition values.
#[derive(Clone, Debug)]
pub(crate) struct DirectoryColumns {
    schema: SchemaRef,
    /// The projection the per-file reader runs, or `None` for every file
    /// column.
    file_columns: Option<Vec<String>>,
    sources: Vec<ColumnSource>,
    partitions: Vec<PartitionColumn>,
}

impl DirectoryColumns {
    /// `columns` is the caller's projection in its order, or `None` for the
    /// whole table. A projection that names no file column still reads one
    /// — the narrowest by compressed bytes in `profile` — for its row
    /// counts, and drops it.
    pub(crate) fn plan(
        file_schema: &SchemaRef,
        layout: &PartitionLayout,
        columns: Option<&[String]>,
        profile: &FooterProfile,
    ) -> Result<Self> {
        let table = table_schema(file_schema, layout);
        let Some(columns) = columns else {
            let sources = (0..file_schema.fields().len())
                .map(ColumnSource::File)
                .chain((0..layout.columns.len()).map(ColumnSource::Partition))
                .collect();
            return Ok(Self {
                schema: table,
                file_columns: None,
                sources,
                partitions: layout.columns.clone(),
            });
        };
        let indices = projection_indices(&table, columns)?;
        let schema = Arc::new(table.project(&indices).map_err(storage_error)?);
        let file_count = file_schema.fields().len();
        let mut file_columns = Vec::new();
        let mut sources = Vec::with_capacity(indices.len());
        for index in indices {
            if index < file_count {
                sources.push(ColumnSource::File(file_columns.len()));
                file_columns.push(file_schema.field(index).name().clone());
            } else {
                sources.push(ColumnSource::Partition(index - file_count));
            }
        }
        if file_columns.is_empty() {
            let narrowest = profile
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| file_schema.field_with_name(&column.name).is_ok())
                .min_by_key(|(position, column)| {
                    (column.compressed_bytes.unwrap_or(u64::MAX), *position)
                })
                .map(|(_, column)| column.name.clone())
                .or_else(|| {
                    file_schema
                        .fields()
                        .first()
                        .map(|field| field.name().clone())
                })
                .ok_or_else(|| error("a Parquet file with no columns cannot be scanned"))?;
            file_columns.push(narrowest);
        }
        Ok(Self {
            schema,
            file_columns: Some(file_columns),
            sources,
            partitions: layout.columns.clone(),
        })
    }

    pub(crate) fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    pub(crate) fn file_columns(&self) -> Option<&[String]> {
        self.file_columns.as_deref()
    }

    /// A file batch, re-wrapped to the advertised schema with the file's
    /// partition values as constant columns.
    pub(crate) fn assemble(
        &self,
        batch: &RecordBatch,
        values: &[PartitionValue],
    ) -> Result<RecordBatch> {
        let rows = batch.num_rows();
        let columns = self
            .sources
            .iter()
            .enumerate()
            .map(|(position, source)| match source {
                ColumnSource::File(index) => Ok(Arc::clone(batch.column(*index))),
                ColumnSource::Partition(index) => values
                    .get(*index)
                    .ok_or_else(|| {
                        error(format!(
                            "partition column '{}' has no value for this file",
                            self.partitions[*index].name()
                        ))
                    })?
                    .array(self.schema.field(position).data_type(), rows),
            })
            .collect::<Result<Vec<_>>>()?;
        RecordBatch::try_new_with_options(
            Arc::clone(&self.schema),
            columns,
            &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(rows)),
        )
        .map_err(storage_error)
    }
}

/// What a Parquet location holds.
#[derive(Debug)]
pub enum ParquetLocation {
    Object(object_store::ObjectMeta),
    Directory(DirectoryListing),
}

/// A directory table on an object store.
#[derive(Clone)]
pub struct ObjectDirectoryReader {
    store: Arc<dyn ObjectStore>,
    account: String,
    container: String,
    root: Path,
    listing: Option<Arc<DirectoryListing>>,
    batch_size: usize,
    columns: Option<Vec<String>>,
    predicate: Option<StoragePredicate>,
    partition: Option<ScanPartition>,
    catalog_schema: Option<SchemaRef>,
    metrics: ScanMetrics,
}

/// One file of a directory scan, ready to open.
struct ScanFile {
    file: DirectoryFile,
    split: bool,
    residual: Option<StoragePredicate>,
    values: Vec<PartitionValue>,
}

impl ObjectDirectoryReader {
    /// `account` and `container` name the per-file cache namespace; for ADLS
    /// they are the real ones so the caches are shared with single-object
    /// reads of the same files.
    pub fn new(
        store: Arc<dyn ObjectStore>,
        account: impl Into<String>,
        container: impl Into<String>,
        root: Path,
    ) -> Self {
        Self {
            store,
            account: account.into(),
            container: container.into(),
            root,
            listing: None,
            batch_size: DEFAULT_BATCH_SIZE,
            columns: None,
            predicate: None,
            partition: None,
            catalog_schema: None,
            metrics: ScanMetrics::default(),
        }
    }

    /// An `abfss://` location through the cached Azure client, an `s3://`
    /// location through the S3 client; credentials come from the environment.
    pub fn from_uri(uri: &str) -> Result<Self> {
        let uri = uri.trim_end_matches('/');
        if uri.starts_with("abfss://") {
            let reader = AdlsParquetReader::from_abfss_uri(uri)?;
            let (account, container, root) = reader.namespace_and_path();
            let store = adls_store(
                &account,
                &container,
                Default::default(),
                &ScanMetrics::default(),
            )?;
            return Ok(Self::new(
                store,
                account,
                container,
                Path::parse(root).map_err(storage_error)?,
            ));
        }
        let bucket = uri
            .strip_prefix("s3://")
            .and_then(|rest| rest.split_once('/'))
            .map(|(bucket, _)| bucket.to_owned())
            .ok_or_else(|| error("expected s3:// or abfss:// object URI"))?;
        let location = ObjectLocation::from_uri(uri)?;
        Ok(Self::new(location.store, "s3", bucket, location.path))
    }

    /// Read a listing already taken for this query instead of listing again.
    pub fn with_listing(mut self, listing: Arc<DirectoryListing>) -> Self {
        self.listing = Some(listing);
        self
    }
    pub fn with_batch_size(mut self, value: usize) -> Self {
        self.batch_size = value;
        self
    }
    pub fn with_columns(mut self, value: Vec<String>) -> Self {
        self.columns = Some(value);
        self
    }
    pub fn with_predicate(mut self, value: StoragePredicate) -> Self {
        self.predicate = Some(match self.predicate.take() {
            Some(previous) => StoragePredicate::And(vec![previous, value]),
            None => value,
        });
        self
    }
    pub fn with_partition(mut self, value: ScanPartition) -> Self {
        self.partition = Some(value);
        self
    }
    /// The schema the catalog serves for the table: a partition column it
    /// names is read as the type it gives, not the inferred one.
    pub fn with_catalog_schema(mut self, value: SchemaRef) -> Self {
        self.catalog_schema = Some(value);
        self
    }
    pub fn with_metrics(mut self, value: ScanMetrics) -> Self {
        self.metrics = value;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> Arc<dyn ObjectStore> {
        Arc::clone(&self.store)
    }

    /// The pinned listing, or the directory listed now.
    pub async fn listing(&self) -> Result<Arc<DirectoryListing>> {
        if let Some(listing) = &self.listing {
            return Ok(Arc::clone(listing));
        }
        let started = std::time::Instant::now();
        let listing = list_parquet_directory(self.store.as_ref(), &self.root).await?;
        self.metrics.snapshot_time(started.elapsed());
        Ok(Arc::new(listing))
    }

    /// Whether the location is one object or a directory of data files. A
    /// `HEAD` that finds no object — ADLS answers that for a directory, S3
    /// for any prefix — is followed by one listing.
    pub async fn probe(&self) -> Result<ParquetLocation> {
        match self.store.head(&self.root).await {
            Ok(object) => Ok(ParquetLocation::Object(object)),
            Err(object_store::Error::NotFound { .. }) => {
                let listing = self.listing().await?;
                if listing.files.is_empty() {
                    return Err(error(format!(
                        "location '{}' is neither an object nor a directory of Parquet files",
                        self.root
                    )));
                }
                Ok(ParquetLocation::Directory(Arc::unwrap_or_clone(listing)))
            }
            Err(failure) => Err(storage_error(failure)),
        }
    }

    fn file_reader(
        &self,
        file: &DirectoryFile,
        columns: Option<&[String]>,
        predicate: Option<&StoragePredicate>,
    ) -> AdlsParquetReader {
        let mut reader = AdlsParquetReader::over_store(
            Arc::clone(&self.store),
            &self.account,
            &self.container,
            file.path.as_ref(),
        )
        .with_batch_size(self.batch_size);
        if let Some(columns) = columns {
            reader = reader.with_columns(columns.to_vec());
        }
        if let Some(predicate) = predicate {
            reader = reader.with_predicate(predicate.clone());
        }
        reader
    }

    /// Exact metadata over every file: the first file's schema with the
    /// partition columns appended, the summed row and row-group counts,
    /// every file checked against the schema.
    pub async fn metadata(&self) -> Result<ParquetFileMetadata> {
        let listing = self.listing().await?;
        let Some(first) = listing.files.first() else {
            return Err(error(format!(
                "location '{}' holds no Parquet data files",
                self.root
            )));
        };
        let layout = PartitionLayout::of(&listing, self.catalog_schema.as_ref())?;
        let metrics = ScanMetrics::default();
        let readers = listing
            .files
            .iter()
            .map(|file| (file.path.clone(), self.file_reader(file, None, None)))
            .collect::<Vec<_>>();
        let mut opened = stream::iter(readers.into_iter().map(|(path, reader)| {
            let store = Arc::clone(&self.store);
            let metrics = metrics.clone();
            async move {
                reader
                    .open(store, &metrics)
                    .await
                    .map(|opened| (path, opened))
                    .map_err(kaveon_core::KaveonError::from)
            }
        }))
        .buffered(METADATA_READ_CONCURRENCY);
        let (_, head) = opened
            .next()
            .await
            .ok_or_else(|| error("directory listing is empty"))??;
        let schema = Arc::clone(head.schema());
        check_partition_columns_absent(
            self.root.as_ref(),
            first.path.as_ref(),
            &schema,
            &layout.columns,
        )?;
        let mut row_count = head.row_count()?;
        let mut row_group_count = head.row_group_count();
        let mut profile = head.profile();
        while let Some(next) = opened.next().await {
            let (path, next) = next?;
            check_file_schema(
                self.root.as_ref(),
                first.path.as_ref(),
                &schema,
                path.as_ref(),
                next.schema(),
            )?;
            row_count = row_count
                .checked_add(next.row_count()?)
                .ok_or_else(|| error("Parquet directory row count overflow"))?;
            row_group_count = row_group_count
                .checked_add(next.row_group_count())
                .ok_or_else(|| error("Parquet directory row-group count overflow"))?;
            profile.merge(next.profile());
        }
        Ok(ParquetFileMetadata {
            schema: table_schema(&schema, &layout),
            row_count,
            row_group_count,
            profile,
        })
    }

    /// Stream the partition's files on a reader thread with its own runtime.
    pub fn read_blocking(self) -> Result<ObjectDirectorySource> {
        if self.batch_size == 0 {
            return Err(error("batch size must be greater than zero"));
        }
        let (initial_tx, initial_rx) = mpsc::sync_channel(1);
        let (tx, rx) = mpsc::sync_channel(2);
        std::thread::Builder::new()
            .name("kaveon-directory-reader".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(scan_parallelism())
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(failure) => {
                        let _ = initial_tx.send(Err(storage_error(failure)));
                        return;
                    }
                };
                runtime.block_on(self.run(initial_tx, tx));
            })
            .map_err(storage_error)?;
        let (schema, metrics) = initial_rx
            .recv()
            .map_err(|_| error("directory reader terminated before initialization"))??;
        Ok(ObjectDirectorySource {
            schema,
            receiver: rx,
            metrics,
            exhausted: false,
        })
    }

    /// The reader loop: list, prune, assign, then one file after another
    /// through the per-object reader. The advertised schema and the metrics
    /// handle go out first; batches follow, re-wrapped to the advertised
    /// schema with the file's partition values; `None` ends the stream and
    /// an error ends it early.
    pub(crate) async fn run(
        self,
        initial_tx: mpsc::SyncSender<Result<(SchemaRef, ScanMetrics)>>,
        tx: mpsc::SyncSender<Result<Option<RecordBatch>>>,
    ) {
        let (columns, files) = match self.prepare().await {
            Ok(prepared) => prepared,
            Err(failure) => {
                let _ = initial_tx.send(Err(failure));
                return;
            }
        };
        if initial_tx
            .send(Ok((Arc::clone(columns.schema()), self.metrics.clone())))
            .is_err()
        {
            return;
        }
        for scan in files {
            let mut reader =
                self.file_reader(&scan.file, columns.file_columns(), scan.residual.as_ref());
            if scan.split
                && let Some(partition) = self.partition
            {
                reader = reader.with_partition(partition);
            }
            let opened = match reader.open(Arc::clone(&self.store), &self.metrics).await {
                Ok(opened) => opened,
                Err(OpenError::NotFound(_)) => {
                    let _ = tx.send(Err(error(format!(
                        "Parquet directory '{}': '{}' disappeared after the directory was listed",
                        self.root, scan.file.path
                    ))));
                    return;
                }
                Err(OpenError::Failed(failure)) => {
                    let _ = tx.send(Err(failure));
                    return;
                }
            };
            self.metrics.file_opened();
            let mut stream = match reader.stream(opened, self.metrics.clone()).await {
                Ok(stream) => stream,
                Err(failure) => {
                    let _ = tx.send(Err(failure));
                    return;
                }
            };
            loop {
                match stream.next_batch().await {
                    Ok(Some(batch)) => {
                        let batch = columns.assemble(&batch, &scan.values);
                        let failed = batch.is_err();
                        if tx.send(batch.map(Some)).is_err() || failed {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(failure) => {
                        let _ = tx.send(Err(failure));
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Ok(None));
    }

    /// The column plan and this partition's files after pruning, every
    /// file's schema checked against the first kept file's as it is opened.
    /// When every file is pruned the first listed file's footer still
    /// supplies the schema (a metadata read; the file is not opened for
    /// rows).
    async fn prepare(&self) -> Result<(DirectoryColumns, Vec<ScanFile>)> {
        let listing = self.listing().await?;
        if listing.files.is_empty() {
            return Err(error(format!(
                "location '{}' is neither an object nor a directory of Parquet files",
                self.root
            )));
        }
        let layout = PartitionLayout::of(&listing, self.catalog_schema.as_ref())?;
        let pruned = prune_files(&layout, self.predicate.as_ref());
        self.metrics
            .files_pruned_by_partition(pruned.pruned_share(self.partition));
        let files = assign_kept_files(&pruned, &layout.sizes, self.partition);
        self.metrics.files_considered(files.len() as u64);
        let first = &listing.files[pruned.kept.first().map_or(0, |file| file.index)];
        let head = self
            .file_reader(first, None, None)
            .open(Arc::clone(&self.store), &self.metrics)
            .await
            .map_err(kaveon_core::KaveonError::from)?;
        let file_schema = Arc::clone(head.schema());
        check_partition_columns_absent(
            self.root.as_ref(),
            first.path.as_ref(),
            &file_schema,
            &layout.columns,
        )?;
        let columns = DirectoryColumns::plan(
            &file_schema,
            &layout,
            self.columns.as_deref(),
            &head.profile(),
        )?;
        let root = self.root.clone();
        let first_path = first.path.clone();
        let store = Arc::clone(&self.store);
        let metrics = self.metrics.clone();
        let files = stream::iter(files.into_iter().map(|(kept, split)| {
            let file = listing.files[kept.index].clone();
            let values = layout.values[kept.index].clone();
            let reader = self.file_reader(&file, None, None);
            let store = Arc::clone(&store);
            let metrics = metrics.clone();
            let (root, first_path, file_schema) =
                (root.clone(), first_path.clone(), Arc::clone(&file_schema));
            async move {
                if file.path != first_path {
                    let opened = reader
                        .open(store, &metrics)
                        .await
                        .map_err(kaveon_core::KaveonError::from)?;
                    check_file_schema(
                        root.as_ref(),
                        first_path.as_ref(),
                        &file_schema,
                        file.path.as_ref(),
                        opened.schema(),
                    )?;
                }
                Ok::<_, kaveon_core::KaveonError>(ScanFile {
                    file,
                    split,
                    residual: kept.residual,
                    values,
                })
            }
        }))
        .buffered(METADATA_READ_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
        Ok((columns, files))
    }
}

pub struct ObjectDirectorySource {
    schema: SchemaRef,
    receiver: mpsc::Receiver<Result<Option<RecordBatch>>>,
    metrics: ScanMetrics,
    exhausted: bool,
}

impl ObjectDirectorySource {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}

impl BatchSource for ObjectDirectorySource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.exhausted {
            return Ok(None);
        }
        let batch = self
            .receiver
            .recv()
            .map_err(|_| error("directory reader terminated without an end-of-stream marker"))??;
        self.exhausted = batch.is_none();
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, AsArray, Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
    };
    use object_store::{PutPayload, memory::InMemory};
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};

    fn parquet_bytes(schema: &SchemaRef, values: Vec<i64>, row_group_size: usize) -> Vec<u8> {
        let labels = values
            .iter()
            .map(|value| format!("v{value}"))
            .collect::<Vec<_>>();
        let mut columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(values))];
        if schema.fields().len() > 1 {
            columns.push(Arc::new(StringArray::from(labels)));
        }
        let batch = RecordBatch::try_new(Arc::clone(schema), columns).unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let mut writer =
            ArrowWriter::try_new(Vec::new(), Arc::clone(schema), Some(properties)).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    fn two_columns() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
        ]))
    }

    async fn put(store: &dyn ObjectStore, path: &str, bytes: Vec<u8>) {
        store
            .put(&Path::parse(path).unwrap(), PutPayload::from(bytes))
            .await
            .unwrap();
    }

    /// A flat directory of three data files with hidden and marker objects
    /// around them: `b` is deliberately written first so the listing order
    /// is the path order, not the write order.
    async fn fixture(store: &dyn ObjectStore) {
        let schema = two_columns();
        put(
            store,
            "table/b.parquet",
            parquet_bytes(&schema, vec![3, 4], 1),
        )
        .await;
        put(
            store,
            "table/a.parquet",
            parquet_bytes(&schema, vec![1, 2], 1),
        )
        .await;
        // The third file is far larger than the other two, so it is the
        // one a two-way partition splits by row group.
        put(
            store,
            "table/c.PARQUET",
            parquet_bytes(&schema, (5..=104).collect(), 10),
        )
        .await;
        put(store, "table/_SUCCESS", Vec::new()).await;
        put(
            store,
            "table/_delta_log/00000000000000000000.json",
            b"{}".to_vec(),
        )
        .await;
        put(
            store,
            "table/.hidden.parquet",
            parquet_bytes(&schema, vec![99], 1),
        )
        .await;
        put(
            store,
            "table/_temporary/x.parquet",
            parquet_bytes(&schema, vec![98], 1),
        )
        .await;
        put(store, "table/empty-marker", Vec::new()).await;
        put(
            store,
            "table-2/other.parquet",
            parquet_bytes(&schema, vec![97], 1),
        )
        .await;
    }

    /// A Hive layout two keys deep: a date and a region, one file in the
    /// NULL date partition, one region value Hive-escaped.
    async fn partitioned_fixture(store: &dyn ObjectStore) {
        let schema = two_columns();
        for (path, values) in [
            ("sales/dt=2026-09-01/region=eu/a.parquet", vec![1, 2]),
            ("sales/dt=2026-09-01/region=us/b.parquet", vec![3]),
            ("sales/dt=2026-09-02/region=eu/c.parquet", vec![4, 5, 6]),
            (
                "sales/dt=__HIVE_DEFAULT_PARTITION__/region=eu/d.parquet",
                vec![7],
            ),
            ("sales/dt=2026-09-02/region=latin%20am/e.parquet", vec![8]),
        ] {
            put(store, path, parquet_bytes(&schema, values, 1)).await;
        }
        put(store, "sales/_SUCCESS", Vec::new()).await;
        put(store, "sales/dt=2026-09-01/_started", Vec::new()).await;
    }

    /// Every `x` of the source, sorted: the decoder lanes interleave row
    /// groups, so arrival order is not file order.
    fn values(source: &mut dyn BatchSource) -> Vec<i64> {
        let mut values = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            let column = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            values.extend(column.values().iter().copied());
        }
        assert!(source.next_batch().unwrap().is_none());
        values.sort_unstable();
        values
    }

    /// Every row as `(x, dt, region)` text, sorted by `x`.
    fn partitioned_rows(
        source: &mut dyn BatchSource,
    ) -> Vec<(i64, Option<String>, Option<String>)> {
        let mut rows = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            assert_eq!(batch.schema(), *source.schema());
            let x = batch
                .column_by_name("x")
                .unwrap()
                .as_primitive::<arrow::datatypes::Int64Type>();
            let dt =
                arrow::compute::cast(batch.column_by_name("dt").unwrap(), &DataType::Utf8).unwrap();
            let dt = dt.as_string::<i32>();
            let region =
                arrow::compute::cast(batch.column_by_name("region").unwrap(), &DataType::Utf8)
                    .unwrap();
            let region = region.as_string::<i32>();
            for row in 0..batch.num_rows() {
                rows.push((
                    x.value(row),
                    dt.is_valid(row).then(|| dt.value(row).to_owned()),
                    region.is_valid(row).then(|| region.value(row).to_owned()),
                ));
            }
        }
        rows.sort();
        rows
    }

    fn compare(column: &str, op: CompareOp, value: ScalarValue) -> StoragePredicate {
        StoragePredicate::Compare {
            column: column.into(),
            op,
            value,
        }
    }

    fn text(value: &str) -> ScalarValue {
        ScalarValue::Utf8(value.into())
    }

    #[test]
    fn verdicts_follow_hive_visibility_and_the_extension_rule() {
        let verdict = |path: &str| file_verdict(path.split('/'));
        assert_eq!(verdict("part-0.parquet"), FileVerdict::Data);
        assert_eq!(verdict("part-0.snappy.PARQUET"), FileVerdict::Data);
        assert_eq!(verdict("20260917_000000_00000_abcde"), FileVerdict::Data);
        assert_eq!(verdict("year=2026/part-0.parquet"), FileVerdict::Data);
        assert_eq!(verdict("_SUCCESS"), FileVerdict::Hidden);
        assert_eq!(verdict("_delta_log/0.json"), FileVerdict::Hidden);
        assert_eq!(verdict(".part-0.parquet.crc"), FileVerdict::Hidden);
        assert_eq!(verdict("_temporary/0/part-0.parquet"), FileVerdict::Hidden);
        assert_eq!(verdict("README.md"), FileVerdict::Foreign);
        assert_eq!(verdict("data/part-0.orc"), FileVerdict::Foreign);
    }

    #[test]
    fn partition_segments_decode_hive_escapes_and_the_null_partition() {
        assert_eq!(hive_decode("plain"), "plain");
        assert_eq!(hive_decode("a%2Fb%3Ac"), "a/b:c");
        assert_eq!(hive_decode("100%25"), "100%");
        assert_eq!(hive_decode("50%"), "50%");
        assert_eq!(hive_decode("%zz"), "%zz");
        assert_eq!(hive_decode("caf%C3%A9"), "café");
        assert_eq!(
            partition_segment("dt=2026-09-01"),
            Some(("dt".into(), Some("2026-09-01".into())))
        );
        assert_eq!(
            partition_segment("dt=__HIVE_DEFAULT_PARTITION__"),
            Some(("dt".into(), None))
        );
        assert_eq!(
            partition_segment("k="),
            Some(("k".into(), Some(String::new())))
        );
        assert_eq!(
            partition_segment("a%3Db=x=y"),
            Some(("a=b".into(), Some("x=y".into())))
        );
        assert_eq!(partition_segment("=v"), None);
        assert_eq!(partition_segment("year2026"), None);
    }

    #[test]
    fn partition_types_are_inferred_from_the_values() {
        assert_eq!(canonical_integer("0"), Some(0));
        assert_eq!(canonical_integer("-12"), Some(-12));
        assert_eq!(canonical_integer("007"), None);
        assert_eq!(canonical_integer("+1"), None);
        assert_eq!(canonical_integer("-0"), None);
        assert_eq!(canonical_integer("1.5"), None);
        assert_eq!(canonical_integer("99999999999999999999"), None);
        assert_eq!(canonical_date("2026-09-01"), Some(20_697));
        assert_eq!(canonical_date("2026-9-1"), None);
        assert_eq!(canonical_date("2026-13-01"), None);
        assert_eq!(canonical_date("2026-09-01T00"), None);
        let infer = |values: &[&str]| infer_partition_type(values.iter().copied());
        assert_eq!(infer(&["1", "22", "-3"]), DataType::Int64);
        assert_eq!(infer(&["2026-09-01", "2026-09-02"]), DataType::Date32);
        assert_eq!(infer(&["2026-09-01", "1"]), DataType::Utf8);
        assert_eq!(infer(&["1", "01"]), DataType::Utf8);
        assert_eq!(infer(&["eu", "us"]), DataType::Utf8);
        assert_eq!(infer(&[]), DataType::Utf8);
        assert_eq!(
            partition_field(&PartitionColumn::new("region", DataType::Utf8).unwrap()).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        assert!(PartitionColumn::new("k", DataType::Float64).is_err());
        assert_eq!(
            PartitionColumn::new(
                "k",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
            )
            .unwrap()
            .data_type(),
            &DataType::Utf8
        );
    }

    #[tokio::test]
    async fn listing_is_sorted_skips_hidden_and_stays_under_the_root() {
        let store = InMemory::new();
        fixture(&store).await;
        let listing = list_parquet_directory(&store, &Path::from("table"))
            .await
            .unwrap();
        assert_eq!(
            listing
                .files
                .iter()
                .map(|file| file.path.to_string())
                .collect::<Vec<_>>(),
            ["table/a.parquet", "table/b.parquet", "table/c.PARQUET"]
        );
        assert!(listing.partitions.is_empty());
        assert!(listing.files.iter().all(|file| file.size > 0));
        assert!(listing.files.iter().all(|file| file.identity().is_some()));
        assert!(
            listing
                .files
                .iter()
                .all(|file| file.partition_values.is_empty())
        );
        assert_eq!(listing.identity_lines().lines().count(), 3);

        put(&store, "table/notes.txt", b"x".to_vec()).await;
        let failure = list_parquet_directory(&store, &Path::from("table"))
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("table/notes.txt"), "{failure}");
    }

    #[tokio::test]
    async fn a_partitioned_listing_carries_typed_keys_and_rejects_mixed_layouts() {
        let store = InMemory::new();
        partitioned_fixture(&store).await;
        let listing = list_parquet_directory(&store, &Path::from("sales"))
            .await
            .unwrap();
        assert_eq!(
            listing.partitions,
            vec![
                PartitionColumn::new("dt", DataType::Date32).unwrap(),
                PartitionColumn::new("region", DataType::Utf8).unwrap(),
            ]
        );
        assert_eq!(
            listing
                .files
                .iter()
                .map(|file| (file.path.to_string(), file.partition_values.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    "sales/dt=2026-09-01/region=eu/a.parquet".to_owned(),
                    vec![Some("2026-09-01".to_owned()), Some("eu".to_owned())]
                ),
                (
                    "sales/dt=2026-09-01/region=us/b.parquet".to_owned(),
                    vec![Some("2026-09-01".to_owned()), Some("us".to_owned())]
                ),
                (
                    "sales/dt=2026-09-02/region=eu/c.parquet".to_owned(),
                    vec![Some("2026-09-02".to_owned()), Some("eu".to_owned())]
                ),
                (
                    "sales/dt=2026-09-02/region=latin%20am/e.parquet".to_owned(),
                    vec![Some("2026-09-02".to_owned()), Some("latin am".to_owned())]
                ),
                (
                    "sales/dt=__HIVE_DEFAULT_PARTITION__/region=eu/d.parquet".to_owned(),
                    vec![None, Some("eu".to_owned())]
                ),
            ]
        );
        // The typed layout: the NULL partition is NULL, the date a day
        // number, and the catalog's type wins where it names a key.
        let layout = PartitionLayout::of(&listing, None).unwrap();
        assert_eq!(
            layout.values[0],
            vec![
                PartitionValue::Date32(20_697),
                PartitionValue::Utf8("eu".into())
            ]
        );
        assert_eq!(
            layout.values[4],
            vec![PartitionValue::Null, PartitionValue::Utf8("eu".into())]
        );
        let as_text = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("dt", DataType::Utf8, true),
        ]));
        let declared = PartitionLayout::of(&listing, Some(&as_text)).unwrap();
        assert_eq!(declared.columns[0].data_type(), &DataType::Utf8);
        assert_eq!(
            declared.values[0][0],
            PartitionValue::Utf8("2026-09-01".into())
        );
        let as_integer = Arc::new(Schema::new(vec![Field::new("dt", DataType::Int64, true)]));
        let failure = PartitionLayout::of(&listing, Some(&as_integer))
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains("sales/dt=2026-09-01/region=eu/a.parquet")
                && failure.contains("bigint"),
            "{failure}"
        );
        let as_float = Arc::new(Schema::new(vec![Field::new("dt", DataType::Float64, true)]));
        let failure = PartitionLayout::of(&listing, Some(&as_float))
            .unwrap_err()
            .to_string();
        assert!(failure.contains("Float64"), "{failure}");

        // A file at another depth names both files.
        let schema = two_columns();
        put(
            &store,
            "sales/dt=2026-09-03/f.parquet",
            parquet_bytes(&schema, vec![9], 1),
        )
        .await;
        let failure = list_parquet_directory(&store, &Path::from("sales"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains("sales/dt=2026-09-03/f.parquet")
                && failure.contains("sales/dt=2026-09-01/region=eu/a.parquet")
                && failure.contains("dt/region"),
            "{failure}"
        );
        store
            .delete(&Path::from("sales/dt=2026-09-03/f.parquet"))
            .await
            .unwrap();
        // Other keys at the same depth, too.
        put(
            &store,
            "sales/dt=2026-09-03/country=fr/g.parquet",
            parquet_bytes(&schema, vec![9], 1),
        )
        .await;
        let failure = list_parquet_directory(&store, &Path::from("sales"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains("g.parquet") && failure.contains("dt/country"),
            "{failure}"
        );
        store
            .delete(&Path::from("sales/dt=2026-09-03/country=fr/g.parquet"))
            .await
            .unwrap();
        // A directory that is not key=value.
        put(
            &store,
            "sales/dt=2026-09-03/region=eu/batch7/h.parquet",
            parquet_bytes(&schema, vec![9], 1),
        )
        .await;
        let failure = list_parquet_directory(&store, &Path::from("sales"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains("h.parquet") && failure.contains("'batch7'"),
            "{failure}"
        );
    }

    #[test]
    fn predicates_fold_over_partition_values() {
        let layout = PartitionLayout {
            columns: vec![
                PartitionColumn::new("dt", DataType::Date32).unwrap(),
                PartitionColumn::new("region", DataType::Utf8).unwrap(),
                PartitionColumn::new("n", DataType::Int64).unwrap(),
            ],
            values: vec![
                vec![
                    PartitionValue::Date32(20_697),
                    PartitionValue::Utf8("eu".into()),
                    PartitionValue::Int64(1),
                ],
                vec![
                    PartitionValue::Date32(20_698),
                    PartitionValue::Utf8("us".into()),
                    PartitionValue::Int64(2),
                ],
                vec![
                    PartitionValue::Null,
                    PartitionValue::Utf8("eu".into()),
                    PartitionValue::Int64(3),
                ],
            ],
            sizes: vec![10, 10, 10],
        };
        let kept = |predicate: StoragePredicate| {
            let pruned = prune_files(&layout, Some(&predicate));
            (
                pruned
                    .kept
                    .iter()
                    .map(|file| (file.index, file.residual.clone()))
                    .collect::<Vec<_>>(),
                pruned.pruned,
            )
        };
        let x_gt = compare("x", CompareOp::Gt, ScalarValue::Int64(5));

        // A text literal against a date key, and the day number a DATE
        // literal lowers to; a NULL key compares to NULL and is pruned.
        assert_eq!(
            kept(compare("dt", CompareOp::Eq, text("2026-09-01"))),
            (vec![(0, None)], vec![1, 2])
        );
        assert_eq!(
            kept(compare("dt", CompareOp::Ge, ScalarValue::Int64(20_698))),
            (vec![(1, None)], vec![0, 2])
        );
        assert_eq!(
            kept(compare("dt", CompareOp::Ne, text("2026-09-01"))),
            (vec![(1, None)], vec![0, 2])
        );
        // IS NULL keeps only the NULL partition; IS NOT NULL the others.
        assert_eq!(
            kept(StoragePredicate::IsNull {
                column: "dt".into()
            }),
            (vec![(2, None)], vec![0, 1])
        );
        assert_eq!(
            kept(StoragePredicate::IsNotNull {
                column: "dt".into()
            }),
            (vec![(0, None), (1, None)], vec![2])
        );
        // IN over text, integer comparison, LIKE.
        assert_eq!(
            kept(StoragePredicate::In {
                column: "region".into(),
                values: vec![text("us"), text("apac")]
            }),
            (vec![(1, None)], vec![0, 2])
        );
        assert_eq!(
            kept(compare("n", CompareOp::Le, ScalarValue::Int64(1))),
            (vec![(0, None)], vec![1, 2])
        );
        assert_eq!(
            kept(StoragePredicate::Like {
                column: "region".into(),
                pattern: "E%".into(),
                negated: false,
                case_insensitive: true
            }),
            (vec![(0, None), (2, None)], vec![1])
        );
        // AND leaves the file-column term as the residual; a satisfied
        // key term is gone from it.
        assert_eq!(
            kept(StoragePredicate::And(vec![
                compare("region", CompareOp::Eq, text("eu")),
                x_gt.clone()
            ])),
            (
                vec![(0, Some(x_gt.clone())), (2, Some(x_gt.clone()))],
                vec![1]
            )
        );
        // OR with a true key term needs no residual; with a false one the
        // file term alone remains; with a NULL one nothing can be pushed.
        assert_eq!(
            kept(StoragePredicate::Or(vec![
                compare("dt", CompareOp::Eq, text("2026-09-01")),
                x_gt.clone()
            ])),
            (vec![(0, None), (1, Some(x_gt.clone())), (2, None)], vec![])
        );
        // NOT over an exactly folded key term flips it; NOT over a term
        // the path cannot decide keeps the file with nothing pushed.
        assert_eq!(
            kept(StoragePredicate::Not(Box::new(compare(
                "region",
                CompareOp::Eq,
                text("eu")
            )))),
            (vec![(1, None)], vec![0, 2])
        );
        assert_eq!(
            kept(StoragePredicate::Not(Box::new(StoragePredicate::And(
                vec![
                    compare("n", CompareOp::Eq, ScalarValue::Float64(1.0)),
                    x_gt.clone()
                ]
            )))),
            (vec![(0, None), (1, None), (2, None)], vec![])
        );
        // NOT over an AND of a key term and a file term is exact: the
        // residual is the negated file term where the key term held.
        assert_eq!(
            kept(StoragePredicate::Not(Box::new(StoragePredicate::And(
                vec![compare("region", CompareOp::Eq, text("eu")), x_gt.clone()]
            )))),
            (
                vec![
                    (0, Some(StoragePredicate::Not(Box::new(x_gt.clone())))),
                    (1, None),
                    (2, Some(StoragePredicate::Not(Box::new(x_gt.clone()))))
                ],
                vec![]
            )
        );
        // A literal of another type decides nothing: every file stays and
        // the term is dropped from the residual.
        assert_eq!(
            kept(StoragePredicate::And(vec![
                compare("n", CompareOp::Eq, text("1")),
                x_gt.clone()
            ])),
            (
                vec![
                    (0, Some(x_gt.clone())),
                    (1, Some(x_gt.clone())),
                    (2, Some(x_gt.clone()))
                ],
                vec![]
            )
        );
        // Without a predicate nothing is pruned; the pruned share deals
        // the pruned files over the scan partitions.
        assert!(prune_files(&layout, None).pruned.is_empty());
        let pruned = prune_files(
            &layout,
            Some(&StoragePredicate::IsNull {
                column: "region".into(),
            }),
        );
        assert_eq!(pruned.pruned, vec![0, 1, 2]);
        assert_eq!(pruned.pruned_share(None), 3);
        assert_eq!(
            pruned.pruned_share(Some(ScanPartition::new(0, 2).unwrap()))
                + pruned.pruned_share(Some(ScanPartition::new(1, 2).unwrap())),
            3
        );
    }

    #[test]
    fn assignment_spreads_whole_files_and_splits_the_large_ones() {
        let partitions = |count: usize| {
            (0..count)
                .map(|index| ScanPartition::new(index, count).unwrap())
                .collect::<Vec<_>>()
        };
        // One partition reads everything whole.
        assert_eq!(
            assign_files(&[5, 5], ScanPartition::new(0, 1).unwrap()),
            FileAssignment {
                whole: vec![0, 1],
                split: vec![]
            }
        );
        // A single file splits by row group on every partition, as before.
        for partition in partitions(3) {
            assert_eq!(
                assign_files(&[100], partition),
                FileAssignment {
                    whole: vec![],
                    split: vec![0]
                }
            );
        }
        // Equal files, one per partition: whole, balanced, no split.
        let equal = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[100, 100, 100], partition))
            .collect::<Vec<_>>();
        assert!(equal.iter().all(|assignment| assignment.split.is_empty()));
        let mut owned = equal
            .iter()
            .flat_map(|assignment| assignment.whole.iter().copied())
            .collect::<Vec<_>>();
        owned.sort_unstable();
        assert_eq!(owned, [0, 1, 2]);
        // Four equal files on three partitions: one would carry twice the
        // fair share, so the largest is split and the rest stay whole.
        let four = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[100, 100, 100, 100], partition))
            .collect::<Vec<_>>();
        assert!(four.iter().all(|assignment| assignment.split == [0]));
        assert!(four.iter().all(|assignment| assignment.whole.len() == 1));
        // One large file with small companions: the large file splits, the
        // small ones spread whole.
        let skewed = partitions(3)
            .into_iter()
            .map(|partition| assign_files(&[10, 300, 10], partition))
            .collect::<Vec<_>>();
        assert!(skewed.iter().all(|assignment| assignment.split == [1]));
        let mut small = skewed
            .iter()
            .flat_map(|assignment| assignment.whole.iter().copied())
            .collect::<Vec<_>>();
        small.sort_unstable();
        assert_eq!(small, [0, 2]);
        // Every file is owned by exactly one partition or split by all.
        for sizes in [
            vec![7, 3, 9, 1, 4, 4, 12, 2],
            vec![1; 17],
            vec![1000, 1, 1, 1],
        ] {
            for count in 1..=5 {
                let assignments = partitions(count)
                    .into_iter()
                    .map(|partition| assign_files(&sizes, partition))
                    .collect::<Vec<_>>();
                for index in 0..sizes.len() {
                    let whole = assignments
                        .iter()
                        .filter(|assignment| assignment.whole.contains(&index))
                        .count();
                    let split = assignments
                        .iter()
                        .filter(|assignment| assignment.split.contains(&index))
                        .count();
                    assert!(
                        (whole == 1 && split == 0) || (whole == 0 && split == count),
                        "{sizes:?} over {count}: file {index} whole {whole} split {split}"
                    );
                }
            }
        }
        // Kept files are assigned by their own sizes: a pruned large file
        // weighs nothing, so the kept small ones stay whole.
        let pruned = PrunedFiles {
            kept: vec![
                KeptFile {
                    index: 0,
                    residual: None,
                },
                KeptFile {
                    index: 2,
                    residual: None,
                },
            ],
            pruned: vec![1],
        };
        let assigned = partitions(2)
            .into_iter()
            .map(|partition| assign_kept_files(&pruned, &[10, 300, 10], Some(partition)))
            .collect::<Vec<_>>();
        assert!(
            assigned
                .iter()
                .all(|files| files.iter().all(|(_, split)| !split))
        );
        let mut owned = assigned
            .iter()
            .flat_map(|files| files.iter().map(|(file, _)| file.index))
            .collect::<Vec<_>>();
        owned.sort_unstable();
        assert_eq!(owned, [0, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn directory_metadata_sums_files_and_checks_every_schema() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        fixture(store.as_ref()).await;
        let reader = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            format!("metadata-{}", uuid_like()),
            Path::from("table"),
        );
        let ParquetLocation::Directory(listing) = reader.probe().await.unwrap() else {
            panic!("a directory of Parquet files is a directory");
        };
        assert_eq!(listing.files.len(), 3);
        let metadata = reader.metadata().await.unwrap();
        assert_eq!(metadata.row_count, 104);
        assert_eq!(metadata.row_group_count, 14);
        assert_eq!(metadata.schema, two_columns());

        // A file with another schema is named, not cast.
        let narrow = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        put(
            store.as_ref(),
            "table/d.parquet",
            parquet_bytes(&narrow, vec![8], 1),
        )
        .await;
        let failure = reader.metadata().await.unwrap_err().to_string();
        assert!(failure.contains("table/d.parquet"), "{failure}");
        assert!(failure.contains("table/a.parquet"), "{failure}");

        // A single object probes as one; nothing at all is an error naming
        // the location.
        let ParquetLocation::Object(object) = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            "probe",
            Path::from("table/a.parquet"),
        )
        .probe()
        .await
        .unwrap() else {
            panic!("an object is an object");
        };
        assert_eq!(object.location, Path::from("table/a.parquet"));
        let missing = ObjectDirectoryReader::new(store, "memory", "probe", Path::from("absent"))
            .probe()
            .await
            .expect_err("nothing at the location is an error")
            .to_string();
        assert!(missing.contains("absent"), "{missing}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn partitioned_metadata_appends_the_keys_and_rejects_a_key_inside_the_files() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        partitioned_fixture(store.as_ref()).await;
        let reader = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            format!("partitioned-metadata-{}", uuid_like()),
            Path::from("sales"),
        );
        let metadata = reader.metadata().await.unwrap();
        assert_eq!(metadata.row_count, 8);
        assert_eq!(
            metadata
                .schema
                .fields()
                .iter()
                .map(|field| (field.name().as_str(), field.data_type().clone()))
                .collect::<Vec<_>>(),
            [
                ("x", DataType::Int64),
                ("label", DataType::Utf8),
                ("dt", DataType::Date32),
                (
                    "region",
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
                ),
            ]
        );
        assert!(metadata.schema.field(2).is_nullable());
        // The catalog's type for a key is the type served.
        let declared = reader
            .clone()
            .with_catalog_schema(Arc::new(Schema::new(vec![Field::new(
                "dt",
                DataType::Utf8,
                true,
            )])))
            .metadata()
            .await
            .unwrap();
        assert_eq!(
            declared.schema.field(2).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );

        // A key that is also a file column has two sources: refused,
        // naming the file, at metadata and at read time.
        let with_region = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("region", DataType::Utf8, false),
        ]));
        let store2: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        put(
            store2.as_ref(),
            "dup/region=eu/a.parquet",
            parquet_bytes(&with_region, vec![1], 1),
        )
        .await;
        let reader = ObjectDirectoryReader::new(
            Arc::clone(&store2),
            "memory",
            format!("duplicate-key-{}", uuid_like()),
            Path::from("dup"),
        );
        let failure = reader.metadata().await.unwrap_err().to_string();
        assert!(
            failure.contains("region") && failure.contains("dup/region=eu/a.parquet"),
            "{failure}"
        );
        let failure = tokio::task::spawn_blocking(move || reader.read_blocking().err())
            .await
            .unwrap()
            .expect("a duplicated key is refused at read time")
            .to_string();
        assert!(failure.contains("region"), "{failure}");
    }

    fn uuid_like() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn partitions_cover_the_directory_once_with_projection_and_pruning() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(fixture(store.as_ref()));
        let container = format!("partitions-{}", uuid_like());
        let mut seen = Vec::new();
        let mut considered = 0;
        for index in 0..2 {
            let metrics = ScanMetrics::default();
            let mut source = ObjectDirectoryReader::new(
                Arc::clone(&store),
                "memory",
                &container,
                Path::from("table"),
            )
            .with_columns(vec!["label".into(), "x".into()])
            .with_partition(ScanPartition::new(index, 2).unwrap())
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
            assert_eq!(source.schema().field(0).name(), "label");
            assert_eq!(source.schema().field(1).name(), "x");
            while let Some(batch) = source.next_batch().unwrap() {
                assert_eq!(batch.schema(), *source.schema());
                let column = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                seen.extend(column.values().iter().copied());
            }
            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.files_opened, snapshot.files_considered);
            assert_eq!(snapshot.files_pruned_by_partition, 0);
            considered += snapshot.files_considered;
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=104).collect::<Vec<_>>());
        // The large file is split between the two partitions and the two
        // small ones spread whole: four file opens in total.
        assert_eq!(considered, 4);

        // Row-group pruning applies inside every file.
        let metrics = ScanMetrics::default();
        let mut pruned = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            &container,
            Path::from("table"),
        )
        .with_predicate(StoragePredicate::Compare {
            column: "x".into(),
            op: kaveon_core::CompareOp::Ge,
            value: kaveon_core::ScalarValue::Int64(100),
        })
        .with_metrics(metrics.clone())
        .read_blocking()
        .unwrap();
        assert_eq!(values(&mut pruned), [100, 101, 102, 103, 104]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_considered, 3);
        assert_eq!(snapshot.row_groups_considered, 14);
        assert_eq!(snapshot.row_groups_selected, 1);

        // A pinned listing is read as pinned: a file added afterwards is not
        // seen until the next query lists again.
        let pinned = runtime
            .block_on(list_parquet_directory(store.as_ref(), &Path::from("table")))
            .unwrap();
        runtime.block_on(put(
            store.as_ref(),
            "table/z.parquet",
            parquet_bytes(&two_columns(), vec![105], 1),
        ));
        let mut from_pin = ObjectDirectoryReader::new(
            Arc::clone(&store),
            "memory",
            &container,
            Path::from("table"),
        )
        .with_listing(Arc::new(pinned))
        .read_blocking()
        .unwrap();
        assert_eq!(values(&mut from_pin), (1..=104).collect::<Vec<_>>());
        let mut fresh =
            ObjectDirectoryReader::new(store, "memory", &container, Path::from("table"))
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut fresh), (1..=105).collect::<Vec<_>>());
    }

    #[test]
    fn a_partitioned_directory_reads_its_keys_as_columns_and_prunes_files() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(partitioned_fixture(store.as_ref()));
        let container = format!("hive-{}", uuid_like());
        let reader = || {
            ObjectDirectoryReader::new(
                Arc::clone(&store),
                "memory",
                &container,
                Path::from("sales"),
            )
        };
        let eu = || Some("eu".to_owned());
        let day1 = || Some("2026-09-01".to_owned());
        let day2 = || Some("2026-09-02".to_owned());

        // Every row carries its path's values; the schema is the files'
        // columns then the keys.
        let metrics = ScanMetrics::default();
        let mut all = reader()
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
        assert_eq!(
            all.schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            ["x", "label", "dt", "region"]
        );
        assert_eq!(
            partitioned_rows(&mut all),
            [
                (1, day1(), eu()),
                (2, day1(), eu()),
                (3, day1(), Some("us".into())),
                (4, day2(), eu()),
                (5, day2(), eu()),
                (6, day2(), eu()),
                (7, None, eu()),
                (8, day2(), Some("latin am".into())),
            ]
        );
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_considered, 5);
        assert_eq!(snapshot.files_opened, 5);
        assert_eq!(snapshot.files_pruned_by_partition, 0);

        // A projection orders keys and file columns as asked.
        let mut projected = reader()
            .with_columns(vec!["region".into(), "x".into(), "dt".into()])
            .read_blocking()
            .unwrap();
        assert_eq!(
            projected
                .schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            ["region", "x", "dt"]
        );
        assert_eq!(partitioned_rows(&mut projected).len(), 8);

        // Only keys projected: the rows still come, one per file row.
        let mut keys_only = reader()
            .with_columns(vec!["dt".into()])
            .read_blocking()
            .unwrap();
        assert_eq!(keys_only.schema().fields().len(), 1);
        let mut rows = 0;
        while let Some(batch) = keys_only.next_batch().unwrap() {
            assert_eq!(batch.num_columns(), 1);
            rows += batch.num_rows();
        }
        assert_eq!(rows, 8);

        // A predicate on the keys prunes before any file is opened: the
        // NULL date file goes with the other date's.
        let metrics = ScanMetrics::default();
        let mut day_one = reader()
            .with_predicate(compare("dt", CompareOp::Eq, text("2026-09-01")))
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
        assert_eq!(values(&mut day_one), [1, 2, 3]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_pruned_by_partition, 3);
        assert_eq!(snapshot.files_considered, 2);
        assert_eq!(snapshot.files_opened, 2);

        // The residual runs inside the kept files: row groups are pruned
        // there, and the key term is not asked of the file.
        let metrics = ScanMetrics::default();
        let mut residual = reader()
            .with_predicate(StoragePredicate::And(vec![
                compare("region", CompareOp::Eq, text("eu")),
                compare("x", CompareOp::Ge, ScalarValue::Int64(5)),
            ]))
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
        assert_eq!(values(&mut residual), [5, 6, 7]);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_pruned_by_partition, 2);
        assert_eq!(snapshot.files_considered, 3);
        assert!(snapshot.row_groups_selected < snapshot.row_groups_considered);

        // Everything pruned: an empty scan with the table's schema.
        let metrics = ScanMetrics::default();
        let mut none = reader()
            .with_predicate(compare("region", CompareOp::Eq, text("apac")))
            .with_metrics(metrics.clone())
            .read_blocking()
            .unwrap();
        assert_eq!(none.schema().fields().len(), 4);
        assert!(values(&mut none).is_empty());
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_pruned_by_partition, 5);
        assert_eq!(snapshot.files_considered, 0);
        assert_eq!(snapshot.files_opened, 0);

        // Over two scan partitions the kept files are spread and the
        // pruned count sums to the total.
        let mut seen = Vec::new();
        let mut pruned_total = 0;
        let mut considered_total = 0;
        let mut opened_total = 0;
        for index in 0..2 {
            let metrics = ScanMetrics::default();
            let mut part = reader()
                .with_predicate(compare("dt", CompareOp::Ge, text("2026-09-02")))
                .with_partition(ScanPartition::new(index, 2).unwrap())
                .with_metrics(metrics.clone())
                .read_blocking()
                .unwrap();
            seen.extend(values(&mut part));
            let snapshot = metrics.snapshot();
            pruned_total += snapshot.files_pruned_by_partition;
            considered_total += snapshot.files_considered;
            opened_total += snapshot.files_opened;
        }
        seen.sort_unstable();
        assert_eq!(seen, [4, 5, 6, 8]);
        assert_eq!(pruned_total, 3);
        // Two small kept files of unequal size are split by row group
        // across both partitions rather than leaving one partition idle;
        // either way only kept files are ever opened.
        assert_eq!(considered_total, opened_total);
        assert!((2..=4).contains(&considered_total));

        // The catalog's type for a key is what the scan produces: `dt` as
        // text compares as text.
        let as_text = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("dt", DataType::Utf8, true),
        ]));
        let mut declared = reader()
            .with_catalog_schema(Arc::clone(&as_text))
            .with_columns(vec!["x".into(), "dt".into(), "region".into()])
            .with_predicate(StoragePredicate::Like {
                column: "dt".into(),
                pattern: "2026-09-0_".into(),
                negated: false,
                case_insensitive: false,
            })
            .read_blocking()
            .unwrap();
        assert_eq!(
            declared.schema().field(1).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        assert_eq!(partitioned_rows(&mut declared).len(), 7);

        // A pinned listing pruned by the planner reads only what survived.
        let listing = runtime
            .block_on(list_parquet_directory(store.as_ref(), &Path::from("sales")))
            .unwrap();
        let pinned = listing
            .pruned_by(None, &compare("region", CompareOp::Eq, text("us")))
            .unwrap();
        assert_eq!(pinned.files.len(), 1);
        assert_eq!(pinned.partitions, listing.partitions);
        let mut from_pin = reader()
            .with_listing(Arc::new(pinned))
            .read_blocking()
            .unwrap();
        assert_eq!(values(&mut from_pin), [3]);
    }

    #[test]
    fn a_schema_mismatch_names_the_file_when_read() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let schema = two_columns();
        runtime.block_on(put(
            store.as_ref(),
            "table/a.parquet",
            parquet_bytes(&schema, vec![1], 1),
        ));
        let retyped = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&retyped),
            vec![
                Arc::new(StringArray::from(vec!["1"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["v1"])) as ArrayRef,
            ],
        )
        .unwrap();
        let mut writer = ArrowWriter::try_new(Vec::new(), retyped, None).unwrap();
        writer.write(&batch).unwrap();
        runtime.block_on(put(
            store.as_ref(),
            "table/b.parquet",
            writer.into_inner().unwrap(),
        ));
        let failure = ObjectDirectoryReader::new(
            store,
            "memory",
            format!("mismatch-{}", uuid_like()),
            Path::from("table"),
        )
        .read_blocking()
        .err()
        .expect("a schema mismatch is an error")
        .to_string();
        assert!(failure.contains("table/b.parquet"), "{failure}");
        assert!(
            failure.contains("Utf8") && failure.contains("Int64"),
            "{failure}"
        );
    }

    #[test]
    fn the_single_object_reader_reads_a_directory_at_its_location() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fixture(store.as_ref()));
        let container = format!("adls-directory-{}", uuid_like());
        let metrics = ScanMetrics::default();
        let mut source =
            AdlsParquetReader::over_store(Arc::clone(&store), "memory", &container, "table")
                .with_columns(vec!["x".into()])
                .with_metrics(metrics.clone())
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut source), (1..=104).collect::<Vec<_>>());
        assert_eq!(metrics.snapshot().files_opened, 3);
        let mut object =
            AdlsParquetReader::over_store(store, "memory", &container, "table/b.parquet")
                .read_blocking()
                .unwrap();
        assert_eq!(values(&mut object), [3, 4]);
    }

    #[test]
    fn the_single_object_reader_prunes_a_partitioned_directory_with_the_catalog_types() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(partitioned_fixture(store.as_ref()));
        let container = format!("adls-hive-{}", uuid_like());
        let metrics = ScanMetrics::default();
        let mut source =
            AdlsParquetReader::over_store(Arc::clone(&store), "memory", &container, "sales")
                .with_columns(vec!["x".into(), "dt".into(), "region".into()])
                .with_predicate(StoragePredicate::In {
                    column: "region".into(),
                    values: vec![text("us"), text("latin am")],
                })
                .with_metrics(metrics.clone())
                .read_blocking()
                .unwrap();
        assert_eq!(
            partitioned_rows(&mut source),
            [
                (3, Some("2026-09-01".into()), Some("us".into())),
                (8, Some("2026-09-02".into()), Some("latin am".into())),
            ]
        );
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.files_pruned_by_partition, 3);
        assert_eq!(snapshot.files_opened, 2);
        let mut declared = AdlsParquetReader::over_store(store, "memory", &container, "sales")
            .with_catalog_schema(Arc::new(Schema::new(vec![Field::new(
                "dt",
                DataType::Utf8,
                true,
            )])))
            .with_columns(vec!["dt".into()])
            .read_blocking()
            .unwrap();
        assert_eq!(
            declared.schema().field(0).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        let mut rows = 0;
        while let Some(batch) = declared.next_batch().unwrap() {
            rows += batch.num_rows();
        }
        assert_eq!(rows, 8);
    }
}
