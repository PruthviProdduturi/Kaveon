//! Building a table's cube from its source — every file read once, its
//! rows folded into every grouping the declared shape names — and the
//! incremental refresh that folds added files in and, from the per-file
//! partials, takes removed files out.
//!
//! The rules:
//! - **Build** (`ANALYZE t WITH (cube = true)`): one scan; each file yields
//!   its cells at every grouping (all measures); the cube is their merge;
//!   the additive measures of each file's cells are kept as its partial.
//! - **Added files**: read once, folded into the cube's cells (sums and
//!   counts add, bounds widen, sketches merge); their partials appended.
//! - **Removed files, additive measures only**: every cell is re-derived
//!   from the remaining partials — sums and counts add up again, bounds
//!   are the bounds of the remaining files — with no read; the added
//!   files of the same refresh are read and folded in.
//! - **Removed files with a distinct-count measure**: a sketch cannot give
//!   a file's values back, and every cell the removed file touched needs
//!   the remaining files' values again — the cube is rebuilt in one scan
//!   (the additive measures come out of the same scan).
//! - **A changed shape, a changed schema, or partials no longer complete**
//!   rebuild.
//! - **Caps**: an axis over its cap in any one file is over it in the
//!   table; its groupings are dropped as soon as one file shows it.

use crate::table_statistics::{DataFile, SourceFiles, enumerate_source};
use arrow::{
    array::{Array, ArrayRef, AsArray},
    compute,
    datatypes::{DataType, TimeUnit},
    record_batch::RecordBatch,
};
use kaveon_core::{
    CubeCell, CubeGrouping, DataFormat, ExcludedAxis, FileCubePartial, KaveonError,
    MeasureAggregate, OperatorMemoryAccount, Result, ShapeAxis, StatValue, TableCube, TableId,
    TableShape,
    cube::{CUBE_PARTIALS_MULTIPLIER, MAX_PER_FILE_CUBES, TABLE_CUBE_VERSION},
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// The estimated resident bytes of one cell while a file is scanned.
const CELL_MEMORY_ESTIMATE: u64 = 192;

/// How the cube is built.
#[derive(Default)]
pub struct CubeBuildOptions {
    /// The account the scan reserves its batches and cells through; none
    /// for a build outside a query.
    pub memory: Option<OperatorMemoryAccount>,
    /// Files read at once; 0 or 1 reads them one after another.
    pub threads: usize,
    /// The most cells the cube may hold (`KAVEON_CUBE_MAX_CELLS`).
    pub max_cells: u64,
}

/// A built or refreshed cube with what the store must do with the
/// partials.
#[derive(Debug)]
pub struct CubeBuild {
    pub cube: TableCube,
    /// The encoded document, already held to the limit.
    pub document: Vec<u8>,
    /// The partials to store: every file's when `replace_partials`, else
    /// the added files'.
    pub partials: Vec<FileCubePartial>,
    pub replace_partials: bool,
    /// Paths whose partials are removed (never with `replace_partials`).
    pub removed_paths: Vec<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the cube from a scan of every file of the source at its current
/// version. The source identity is read again at the end; a source that
/// changed underneath fails rather than mixing versions.
pub fn build_cube(
    location: &str,
    format: DataFormat,
    table_id: TableId,
    shape: &TableShape,
    options: &CubeBuildOptions,
) -> Result<CubeBuild> {
    let source = enumerate_source(location, format)?;
    build_from_source(&source, location, format, table_id, shape, options)
}

fn build_from_source(
    source: &SourceFiles,
    location: &str,
    format: DataFormat,
    table_id: TableId,
    shape: &TableShape,
    options: &CubeBuildOptions,
) -> Result<CubeBuild> {
    if shape.is_empty() {
        return Err(KaveonError::Execution(
            "the table declares no shape; ALTER TABLE … SET SHAPE (…) first".into(),
        ));
    }
    let scans = scan_files(&source.files, shape, options)?;
    let mut cube = empty_cube(table_id, shape, source);
    let mut partials = Vec::with_capacity(scans.len());
    for scan in scans {
        fold_scan(&mut cube, &scan)?;
        partials.push(scan.partial(&cube));
    }
    finish(&mut cube, &mut partials, location, format, source, options)?;
    let document = cube.check_limit(options.max_cells)?;
    Ok(CubeBuild {
        cube,
        document,
        partials,
        replace_partials: true,
        removed_paths: Vec::new(),
    })
}

/// The cube for the source's current version, from `previous` and its
/// partials: the same version comes back unchanged (`None`); added files
/// are read and folded in; removed files are taken out from the partials
/// when every measure is additive, else the cube is rebuilt; a changed
/// shape, schema or incomplete partials rebuild.
pub fn refresh_cube(
    previous: &TableCube,
    partials: &[FileCubePartial],
    location: &str,
    format: DataFormat,
    shape: &TableShape,
    options: &CubeBuildOptions,
) -> Result<Option<CubeBuild>> {
    let source = enumerate_source(location, format)?;
    if source.profile.statistics.identity_sha256 == previous.source_version.identity_sha256 {
        return Ok(None);
    }
    let table_id = previous.table_id.clone();
    let rebuild = |source: &SourceFiles| {
        build_from_source(source, location, format, table_id.clone(), shape, options).map(Some)
    };
    let known: BTreeSet<&str> = previous.files.iter().map(String::as_str).collect();
    let on_record: BTreeSet<&str> = partials.iter().map(|p| p.path.as_str()).collect();
    let current: BTreeSet<&str> = source.files.iter().map(|f| f.label.as_str()).collect();
    let removed: Vec<String> = known
        .iter()
        .filter(|path| !current.contains(*path))
        .map(|path| (*path).to_owned())
        .collect();
    let partials_complete = previous.per_file_complete && known == on_record;
    if &previous.shape != shape || !partials_complete {
        return rebuild(&source);
    }
    if !removed.is_empty() && previous.shape.has_non_additive() {
        return rebuild(&source);
    }
    let added: Vec<DataFile> = source
        .files
        .iter()
        .filter(|file| !known.contains(file.label.as_str()))
        .cloned()
        .collect();
    let scans = scan_files(&added, shape, options)?;
    let mut cube = if removed.is_empty() {
        // The added files fold into the cube as it stands.
        let mut cube = previous.clone();
        cube.files.clear();
        cube
    } else {
        // Every cell from the remaining partials, then the added files.
        let mut cube = empty_cube(table_id, shape, &source);
        for partial in partials
            .iter()
            .filter(|partial| !removed.contains(&partial.path))
        {
            fold_partial(&mut cube, partial)?;
        }
        cube
    };
    let mut new_partials = Vec::with_capacity(scans.len());
    for scan in scans {
        fold_scan(&mut cube, &scan)?;
        new_partials.push(scan.partial(&cube));
    }
    // The excluded axes of the previous cube stay excluded; a rebuild
    // re-evaluates them.
    for excluded in &previous.excluded {
        if !cube.excluded.iter().any(|e| e.column == excluded.column) {
            cube.excluded.push(excluded.clone());
        }
    }
    let mut all_partials: Vec<FileCubePartial> = partials
        .iter()
        .filter(|partial| !removed.contains(&partial.path))
        .cloned()
        .collect();
    all_partials.extend(new_partials.iter().cloned());
    finish(
        &mut cube,
        &mut all_partials,
        location,
        format,
        &source,
        options,
    )?;
    let document = cube.check_limit(options.max_cells)?;
    if !cube.per_file_complete {
        // The partials outgrew their bound: none are kept.
        return Ok(Some(CubeBuild {
            cube,
            document,
            partials: Vec::new(),
            replace_partials: true,
            removed_paths: Vec::new(),
        }));
    }
    // `finish` stripped the groupings of excluded axes from every partial;
    // the new ones are stored as stripped.
    let new_paths: BTreeSet<&str> = new_partials.iter().map(|p| p.path.as_str()).collect();
    let new_partials = all_partials
        .into_iter()
        .filter(|partial| new_paths.contains(partial.path.as_str()))
        .collect();
    Ok(Some(CubeBuild {
        cube,
        document,
        partials: new_partials,
        replace_partials: false,
        removed_paths: removed,
    }))
}

fn empty_cube(table_id: TableId, shape: &TableShape, source: &SourceFiles) -> TableCube {
    TableCube {
        version: TABLE_CUBE_VERSION,
        table_id,
        source_version: source.source_version(),
        computed_at_ms: 0,
        slots: shape.measure_slots(),
        shape: shape.clone(),
        groupings: shape
            .groupings()
            .into_iter()
            .map(|axes| CubeGrouping {
                axes,
                cells: Vec::new(),
            })
            .collect(),
        excluded: Vec::new(),
        files: Vec::new(),
        per_file_complete: true,
    }
}

/// Apply the caps, order the files, re-read the identity, bound the
/// partials, stamp the time.
fn finish(
    cube: &mut TableCube,
    partials: &mut Vec<FileCubePartial>,
    location: &str,
    format: DataFormat,
    source: &SourceFiles,
    options: &CubeBuildOptions,
) -> Result<()> {
    cube.apply_caps();
    let dropped: Vec<usize> = cube
        .axes()
        .iter()
        .enumerate()
        .filter(|(_, axis)| cube.excluded.iter().any(|e| e.column == axis.column))
        .map(|(index, _)| index)
        .collect();
    for partial in partials.iter_mut() {
        partial
            .groupings
            .retain(|grouping| !grouping.axes.iter().any(|axis| dropped.contains(axis)));
    }
    cube.files = source.files.iter().map(|file| file.label.clone()).collect();
    let partial_cells: u64 = partials.iter().map(FileCubePartial::cell_count).sum();
    cube.per_file_complete = partials.len() == cube.files.len()
        && partials.len() <= MAX_PER_FILE_CUBES
        && partial_cells <= options.max_cells.saturating_mul(CUBE_PARTIALS_MULTIPLIER);
    if !cube.per_file_complete {
        partials.clear();
    }
    let after = crate::analyze_source(location, format)?;
    if after.identity_sha256 != source.profile.statistics.identity_sha256 {
        return Err(KaveonError::Storage(
            "table source changed while its cube was being built".into(),
        ));
    }
    cube.source_version = source.source_version();
    cube.computed_at_ms = now_ms();
    Ok(())
}

/// Fold one file's scan into the cube: every grouping the scan kept (a
/// grouping the file dropped for an over-cap axis excludes the axis).
fn fold_scan(cube: &mut TableCube, scan: &FileScan) -> Result<()> {
    let axes = cube.axes();
    for (index, observed) in &scan.over_cap {
        let axis = &axes[*index];
        if let Some(excluded) = cube.excluded.iter_mut().find(|e| e.column == axis.column) {
            excluded.distinct = excluded.distinct.max(*observed);
        } else {
            cube.excluded.push(ExcludedAxis {
                column: axis.column.clone(),
                distinct: *observed,
                cap: axis.cap,
            });
        }
    }
    for grouping in &scan.groupings {
        if let Some(mine) = cube
            .groupings
            .iter_mut()
            .find(|mine| mine.axes == grouping.axes)
        {
            mine.fold_in(grouping)?;
        }
    }
    Ok(())
}

/// Fold one file's partial (additive slots only) into a cube whose cells
/// carry every slot: the non-additive slots stay empty, which is only
/// right while no non-additive measure is declared (the caller's rule).
fn fold_partial(cube: &mut TableCube, partial: &FileCubePartial) -> Result<()> {
    let additive = cube.additive_slots();
    if additive.len() != cube.slots.len() {
        return Err(KaveonError::Execution(
            "a cube with a distinct-count measure is not re-derived from partials".into(),
        ));
    }
    for grouping in &partial.groupings {
        if let Some(mine) = cube
            .groupings
            .iter_mut()
            .find(|mine| mine.axes == grouping.axes)
        {
            mine.fold_in(grouping)?;
        }
    }
    Ok(())
}

/// What one file's scan found: its cells at every grouping it kept, with
/// every measure slot.
struct FileScan {
    path: String,
    rows: u64,
    groupings: Vec<CubeGrouping>,
    /// Axes the file showed over their cap, with the count observed.
    over_cap: Vec<(usize, u64)>,
}

impl FileScan {
    /// The file's partial: the additive slots of every cell.
    fn partial(&self, cube: &TableCube) -> FileCubePartial {
        let additive = cube.additive_slots();
        FileCubePartial {
            path: self.path.clone(),
            rows: self.rows,
            groupings: self
                .groupings
                .iter()
                .map(|grouping| CubeGrouping {
                    axes: grouping.axes.clone(),
                    cells: grouping
                        .cells
                        .iter()
                        .map(|cell| CubeCell {
                            key: cell.key.clone(),
                            rows: cell.rows,
                            measures: additive
                                .iter()
                                .map(|slot| cell.measures[*slot].clone())
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// Read every file, `options.threads` at a time.
fn scan_files(
    files: &[DataFile],
    shape: &TableShape,
    options: &CubeBuildOptions,
) -> Result<Vec<FileScan>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let results: Vec<Mutex<Option<Result<FileScan>>>> =
        files.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = options.threads.clamp(1, files.len());
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if failed.load(AtomicOrdering::Relaxed) {
                        return;
                    }
                    let index = next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(file) = files.get(index) else {
                        return;
                    };
                    let result = scan_file(file, shape, options.memory.as_ref());
                    if result.is_err() {
                        failed.store(true, AtomicOrdering::Relaxed);
                    }
                    *results[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_else(|| Err(KaveonError::Storage("cube scan did not run".into())))
        })
        .collect()
}

/// The values of one axis in a file, interned so a row's key along a
/// grouping is a few small integers.
struct AxisValues {
    codes: HashMap<Option<String>, u32>,
    values: Vec<Option<StatValue>>,
}

impl AxisValues {
    fn code(&mut self, value: Option<StatValue>) -> u32 {
        let text = value.as_ref().map(StatValue::to_hash_text);
        if let Some(code) = self.codes.get(&text) {
            return *code;
        }
        let code = self.values.len() as u32;
        self.codes.insert(text, code);
        self.values.push(value);
        code
    }
}

/// The cells of one grouping while a file is scanned, keyed by the axis
/// codes.
struct GroupingScan {
    axes: Vec<usize>,
    cells: HashMap<Vec<u32>, CubeCell>,
    dropped: bool,
}

fn scan_file(
    file: &DataFile,
    shape: &TableShape,
    memory: Option<&OperatorMemoryAccount>,
) -> Result<FileScan> {
    let axes = shape.axes();
    let slots = shape.measure_slots();
    let slot_kinds: Vec<MeasureAggregate> = slots.iter().map(|(_, kind)| *kind).collect();
    let mut columns: Vec<String> = axes.iter().map(|axis| axis.column.clone()).collect();
    for (column, _) in &slots {
        if !columns.contains(column) {
            columns.push(column.clone());
        }
    }
    let mut source = file.open(Some(&columns))?;
    let mut axis_values: Vec<AxisValues> = axes
        .iter()
        .map(|_| AxisValues {
            codes: HashMap::new(),
            values: Vec::new(),
        })
        .collect();
    let mut groupings: Vec<GroupingScan> = shape
        .groupings()
        .into_iter()
        .map(|axes| GroupingScan {
            axes,
            cells: HashMap::new(),
            dropped: false,
        })
        .collect();
    let mut over_cap: Vec<(usize, u64)> = Vec::new();
    let mut rows = 0u64;
    let mut cells_reserved = 0u64;
    let mut cell_reservations = Vec::new();
    let mut row_codes: Vec<u32> = vec![0; axes.len()];
    let mut key = Vec::with_capacity(2);
    while let Some(batch) = source.next_batch()? {
        let batch_reservation = memory
            .map(|memory| {
                memory.check_cancelled()?;
                memory.reserve(batch.get_array_memory_size() as u64)
            })
            .transpose()?;
        rows += batch.num_rows() as u64;
        let axis_readers = axes
            .iter()
            .map(|axis| RowReader::new(&batch, &axis.column, file))
            .collect::<Result<Vec<_>>>()?;
        let measure_readers = slots
            .iter()
            .map(|(column, _)| RowReader::new(&batch, column, file))
            .collect::<Result<Vec<_>>>()?;
        for row in 0..batch.num_rows() {
            for (index, (axis, reader)) in axes.iter().zip(&axis_readers).enumerate() {
                if over_cap.iter().any(|(over, _)| *over == index) {
                    continue;
                }
                let value = axis_value(axis, reader.at(row))?;
                row_codes[index] = axis_values[index].code(value);
            }
            for grouping in groupings.iter_mut().filter(|grouping| !grouping.dropped) {
                key.clear();
                key.extend(grouping.axes.iter().map(|axis| row_codes[*axis]));
                let cell = match grouping.cells.get_mut(&key) {
                    Some(cell) => cell,
                    None => {
                        let stat_key = grouping
                            .axes
                            .iter()
                            .map(|axis| {
                                axis_values[*axis].values[row_codes[*axis] as usize].clone()
                            })
                            .collect();
                        grouping
                            .cells
                            .entry(key.clone())
                            .or_insert_with(|| CubeCell::empty(stat_key, &slot_kinds))
                    }
                };
                cell.rows += 1;
                for (measure, reader) in cell.measures.iter_mut().zip(&measure_readers) {
                    if let Some(value) = reader.at(row) {
                        measure.fold_value(&value)?;
                    }
                }
            }
            // An axis over its cap: its groupings are dropped from here on.
            for (index, axis) in axes.iter().enumerate() {
                if over_cap.iter().any(|(over, _)| *over == index) {
                    continue;
                }
                let distinct = axis_values[index].values.len() as u64;
                if distinct > axis.cap {
                    over_cap.push((index, distinct));
                    for grouping in &mut groupings {
                        if grouping.axes.contains(&index) {
                            grouping.dropped = true;
                            grouping.cells = HashMap::new();
                        }
                    }
                    axis_values[index] = AxisValues {
                        codes: HashMap::new(),
                        values: Vec::new(),
                    };
                }
            }
        }
        if let Some(memory) = memory {
            let cells: u64 = groupings.iter().map(|g| g.cells.len() as u64).sum();
            if cells > cells_reserved {
                cell_reservations
                    .push(memory.reserve((cells - cells_reserved) * CELL_MEMORY_ESTIMATE)?);
                cells_reserved = cells;
            }
        }
        drop(batch_reservation);
    }
    let groupings = groupings
        .into_iter()
        .filter(|grouping| !grouping.dropped)
        .map(|grouping| {
            let mut cells: Vec<CubeCell> = grouping.cells.into_values().collect();
            cells.sort_by(|a, b| kaveon_core::cube::compare_keys(&a.key, &b.key));
            CubeGrouping {
                axes: grouping.axes,
                cells,
            }
        })
        .collect();
    drop(cell_reservations);
    Ok(FileScan {
        path: file.label.clone(),
        rows,
        groupings,
        over_cap,
    })
}

/// A dimension value as the key holds it; the time axis at its grain.
fn axis_value(axis: &ShapeAxis, value: Option<StatValue>) -> Result<Option<StatValue>> {
    match (axis.grain, value) {
        (_, None) => Ok(None),
        (None, Some(value)) => Ok(Some(value)),
        (Some(grain), Some(value)) => grain.truncate(&value).map(Some).ok_or_else(|| {
            KaveonError::Execution(format!(
                "time '{}' holds {value:?}, which {} grain does not bucket",
                axis.column,
                grain.name()
            ))
        }),
    }
}

/// One column of a batch, read row by row as logical values.
enum RowReader {
    Bool(arrow::array::BooleanArray),
    Int(ArrayRef),
    Float(ArrayRef),
    Text(ArrayRef),
    Date(arrow::array::Date32Array),
    Timestamp {
        values: arrow::array::PrimitiveArray<arrow::datatypes::Int64Type>,
        unit: TimeUnit,
        utc: bool,
    },
    Decimal {
        values: arrow::array::Decimal128Array,
        scale: i8,
    },
}

impl RowReader {
    fn new(batch: &RecordBatch, column: &str, file: &DataFile) -> Result<Self> {
        let array = batch.column_by_name(column).ok_or_else(|| {
            KaveonError::Storage(format!("column '{column}' missing from '{}'", file.label))
        })?;
        let array: ArrayRef = match array.data_type() {
            DataType::Dictionary(_, values) => compute::cast(array, values)?,
            _ => std::sync::Arc::clone(array),
        };
        Ok(match array.data_type() {
            DataType::Boolean => Self::Bool(array.as_boolean().clone()),
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32 => Self::Int(compute::cast(&array, &DataType::Int64)?),
            DataType::UInt64 => Self::Int(array),
            DataType::Float32 | DataType::Float64 => {
                Self::Float(compute::cast(&array, &DataType::Float64)?)
            }
            DataType::Utf8 | DataType::LargeUtf8 => Self::Text(array),
            DataType::Date32 => {
                Self::Date(array.as_primitive::<arrow::datatypes::Date32Type>().clone())
            }
            DataType::Timestamp(unit, zone) => Self::Timestamp {
                values: kaveon_core::sketch::timestamp_values(&array, *unit),
                unit: *unit,
                utc: zone.is_some(),
            },
            DataType::Decimal128(_, scale) => Self::Decimal {
                values: array
                    .as_primitive::<arrow::datatypes::Decimal128Type>()
                    .clone(),
                scale: *scale,
            },
            other => {
                return Err(KaveonError::Storage(format!(
                    "column '{column}' ({other}) cannot be a cube axis or measure"
                )));
            }
        })
    }

    fn at(&self, row: usize) -> Option<StatValue> {
        match self {
            Self::Bool(values) => {
                (!values.is_null(row)).then(|| StatValue::Bool(values.value(row)))
            }
            Self::Int(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(StatValue::Int(match values.data_type() {
                    DataType::UInt64 => i128::from(
                        values
                            .as_primitive::<arrow::datatypes::UInt64Type>()
                            .value(row),
                    ),
                    _ => i128::from(
                        values
                            .as_primitive::<arrow::datatypes::Int64Type>()
                            .value(row),
                    ),
                }))
            }
            Self::Float(values) => (!values.is_null(row)).then(|| {
                StatValue::Float(
                    values
                        .as_primitive::<arrow::datatypes::Float64Type>()
                        .value(row),
                )
            }),
            Self::Text(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(StatValue::Text(match values.data_type() {
                    DataType::LargeUtf8 => values.as_string::<i64>().value(row).to_owned(),
                    _ => values.as_string::<i32>().value(row).to_owned(),
                }))
            }
            Self::Date(values) => {
                (!values.is_null(row)).then(|| StatValue::Date(values.value(row)))
            }
            Self::Timestamp { values, unit, utc } => {
                (!values.is_null(row)).then(|| StatValue::Timestamp {
                    value: values.value(row),
                    unit: *unit,
                    utc: *utc,
                })
            }
            Self::Decimal { values, scale } => (!values.is_null(row)).then(|| StatValue::Decimal {
                unscaled: values.value(row),
                scale: *scale,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Date32Array, Float64Array, Int64Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use kaveon_core::{CellMeasure, ShapeAxis, TableShape};
    use std::sync::Arc;

    fn schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("region", DataType::Utf8, true),
            Field::new("status", DataType::Utf8, true),
            Field::new("total", DataType::Int64, true),
            Field::new("score", DataType::Float64, true),
            Field::new("user_id", DataType::Int64, true),
            Field::new("day", DataType::Date32, true),
        ]))
    }

    /// `rows` rows from `start`: region cycles over four values (null
    /// every 11th), status over three, total is the id, score half the
    /// id (null every 7th), user_id the id modulo 50, day one of five.
    fn write_file(path: &std::path::Path, start: i64, rows: i64) {
        let ids: Vec<i64> = (start..start + rows).collect();
        let regions = ["EU", "US", "APAC", "LATAM"];
        let statuses = ["open", "closed", "hold"];
        let batch = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(StringArray::from(
                    ids.iter()
                        .map(|id| (id % 11 != 0).then(|| regions[(*id % 4) as usize]))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    ids.iter()
                        .map(|id| Some(statuses[(*id % 3) as usize]))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from(ids.clone())),
                Arc::new(Float64Array::from(
                    ids.iter()
                        .map(|id| (id % 7 != 0).then_some(*id as f64 * 0.5))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from(
                    ids.iter().map(|id| id % 50).collect::<Vec<_>>(),
                )),
                Arc::new(Date32Array::from(
                    ids.iter()
                        .map(|id| 20_000 + (*id % 5) as i32)
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap();
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(path).unwrap(),
            schema(),
            None,
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    fn shape() -> TableShape {
        TableShape::parse(
            &["region:10".into(), "status:10".into()],
            &[
                "total:sum,count,min,max".into(),
                "score:sum".into(),
                "user_id:count_distinct".into(),
            ],
            Some("day:day:100"),
        )
        .unwrap()
    }

    fn additive_shape() -> TableShape {
        TableShape::parse(
            &["region:10".into(), "status:10".into()],
            &["total:sum,count,min,max".into(), "score:sum".into()],
            Some("day:day:100"),
        )
        .unwrap()
    }

    fn options() -> CubeBuildOptions {
        CubeBuildOptions {
            memory: None,
            threads: 2,
            max_cells: 10_000,
        }
    }

    fn directory(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let directory = std::env::temp_dir().join(format!(
            "kaveon-cube-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn table_id() -> TableId {
        TableId::new("table:lake:sales:events").unwrap()
    }

    /// The cell of `grouping` at `key`.
    fn cell<'a>(cube: &'a TableCube, axes: &[usize], key: &[Option<StatValue>]) -> &'a CubeCell {
        cube.grouping(axes)
            .unwrap()
            .cells
            .iter()
            .find(|cell| cell.key == key)
            .unwrap_or_else(|| panic!("cell {key:?} of {axes:?}"))
    }

    #[test]
    fn a_build_holds_every_grouping_with_exact_additive_measures() {
        let directory = directory("build");
        let location = directory.join("events");
        std::fs::create_dir_all(&location).unwrap();
        write_file(&location.join("a.parquet"), 0, 100);
        write_file(&location.join("b.parquet"), 100, 100);
        let built = build_cube(
            location.to_str().unwrap(),
            DataFormat::Parquet,
            table_id(),
            &shape(),
            &options(),
        )
        .unwrap();
        let cube = &built.cube;
        assert!(built.replace_partials);
        assert_eq!(built.partials.len(), 2);
        assert!(cube.per_file_complete);
        assert_eq!(cube.files, ["a.parquet", "b.parquet"]);
        assert!(cube.excluded.is_empty());
        // Groupings: (), region, status, day, region×status, region×day,
        // status×day.
        assert_eq!(cube.groupings.len(), 7);
        let total = cell(cube, &[], &[]);
        assert_eq!(total.rows, 200);
        assert_eq!(
            total.measures[0],
            CellMeasure::Sum(Some(StatValue::Int((0..200).sum())))
        );
        assert_eq!(total.measures[1], CellMeasure::Count(200));
        assert_eq!(total.measures[2], CellMeasure::Min(Some(StatValue::Int(0))));
        assert_eq!(
            total.measures[3],
            CellMeasure::Max(Some(StatValue::Int(199)))
        );
        let expected_score: f64 = (0..200)
            .filter(|id| id % 7 != 0)
            .map(|id| id as f64 * 0.5)
            .sum();
        assert_eq!(
            total.measures[4],
            CellMeasure::Sum(Some(StatValue::Float(expected_score)))
        );
        let CellMeasure::Distinct(sketch) = &total.measures[5] else {
            panic!("distinct");
        };
        assert!(
            (48..=52).contains(&sketch.estimate()),
            "{}",
            sketch.estimate()
        );
        // A null region is a key of its own.
        let nulls = cell(cube, &[0], &[None]);
        assert_eq!(
            nulls.rows,
            (0..200).filter(|id| id % 11 == 0).count() as u64
        );
        let eu_open = cell(
            cube,
            &[0, 1],
            &[
                Some(StatValue::Text("EU".into())),
                Some(StatValue::Text("open".into())),
            ],
        );
        let expected: Vec<i64> = (0..200)
            .filter(|id| id % 11 != 0 && id % 4 == 0 && id % 3 == 0)
            .collect();
        assert_eq!(eu_open.rows, expected.len() as u64);
        assert_eq!(
            eu_open.measures[0],
            CellMeasure::Sum(Some(StatValue::Int(expected.iter().sum::<i64>() as i128)))
        );
        assert_eq!(cube.grouping(&[2]).unwrap().cells.len(), 5);
        // The partial carries the additive slots only.
        assert_eq!(built.partials[0].groupings[0].cells[0].measures.len(), 5);
        assert_eq!(built.partials[0].rows, 100);
        // The document round-trips.
        assert_eq!(&TableCube::from_json_bytes(&built.document).unwrap(), cube);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_added_file_folds_in_to_what_a_rebuild_computes() {
        let directory = directory("fold");
        let location = directory.join("events");
        std::fs::create_dir_all(&location).unwrap();
        write_file(&location.join("a.parquet"), 0, 100);
        write_file(&location.join("b.parquet"), 100, 100);
        let path = location.to_str().unwrap();
        let first =
            build_cube(path, DataFormat::Parquet, table_id(), &shape(), &options()).unwrap();
        assert!(
            refresh_cube(
                &first.cube,
                &first.partials,
                path,
                DataFormat::Parquet,
                &shape(),
                &options()
            )
            .unwrap()
            .is_none()
        );
        write_file(&location.join("c.parquet"), 200, 100);
        let folded = refresh_cube(
            &first.cube,
            &first.partials,
            path,
            DataFormat::Parquet,
            &shape(),
            &options(),
        )
        .unwrap()
        .expect("a new version");
        assert!(!folded.replace_partials);
        assert_eq!(folded.partials.len(), 1);
        assert_eq!(folded.partials[0].path, "c.parquet");
        assert!(folded.removed_paths.is_empty());
        let rebuilt =
            build_cube(path, DataFormat::Parquet, table_id(), &shape(), &options()).unwrap();
        assert_eq!(folded.cube.source_version, rebuilt.cube.source_version);
        assert_eq!(folded.cube.files, rebuilt.cube.files);
        assert_eq!(folded.cube.groupings, rebuilt.cube.groupings);
        assert!(folded.cube.per_file_complete);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_removed_file_is_taken_out_from_the_partials_or_rebuilds_with_a_distinct_measure() {
        let directory = directory("remove");
        let location = directory.join("events");
        std::fs::create_dir_all(&location).unwrap();
        write_file(&location.join("a.parquet"), 0, 100);
        write_file(&location.join("b.parquet"), 100, 100);
        write_file(&location.join("c.parquet"), 200, 100);
        let path = location.to_str().unwrap();
        // Additive measures only: the removal re-derives from partials and
        // the added file is read.
        let first = build_cube(
            path,
            DataFormat::Parquet,
            table_id(),
            &additive_shape(),
            &options(),
        )
        .unwrap();
        std::fs::remove_file(location.join("b.parquet")).unwrap();
        write_file(&location.join("d.parquet"), 300, 50);
        let refreshed = refresh_cube(
            &first.cube,
            &first.partials,
            path,
            DataFormat::Parquet,
            &additive_shape(),
            &options(),
        )
        .unwrap()
        .expect("a new version");
        assert!(!refreshed.replace_partials);
        assert_eq!(refreshed.removed_paths, ["b.parquet"]);
        assert_eq!(refreshed.partials.len(), 1);
        assert_eq!(refreshed.partials[0].path, "d.parquet");
        let rebuilt = build_cube(
            path,
            DataFormat::Parquet,
            table_id(),
            &additive_shape(),
            &options(),
        )
        .unwrap();
        assert_eq!(refreshed.cube.groupings, rebuilt.cube.groupings);
        assert_eq!(
            refreshed.cube.files,
            ["a.parquet", "c.parquet", "d.parquet"]
        );
        let total = cell(&refreshed.cube, &[], &[]);
        assert_eq!(total.rows, 250);
        assert_eq!(
            total.measures[3],
            CellMeasure::Max(Some(StatValue::Int(349)))
        );

        // With a distinct-count measure a removal rebuilds: every partial
        // is replaced.
        let with_distinct =
            build_cube(path, DataFormat::Parquet, table_id(), &shape(), &options()).unwrap();
        std::fs::remove_file(location.join("c.parquet")).unwrap();
        let rebuilt = refresh_cube(
            &with_distinct.cube,
            &with_distinct.partials,
            path,
            DataFormat::Parquet,
            &shape(),
            &options(),
        )
        .unwrap()
        .expect("a new version");
        assert!(rebuilt.replace_partials);
        assert_eq!(rebuilt.partials.len(), 2);
        assert_eq!(rebuilt.cube.files, ["a.parquet", "d.parquet"]);
        assert_eq!(cell(&rebuilt.cube, &[], &[]).rows, 150);
        // A changed shape rebuilds too.
        let reshaped = refresh_cube(
            &rebuilt.cube,
            &rebuilt.partials,
            path,
            DataFormat::Parquet,
            &additive_shape(),
            &options(),
        );
        // Same version: nothing to do until the shape is applied by a
        // build; a refresh at the same identity is `None`.
        assert!(reshaped.unwrap().is_none());
        write_file(&location.join("e.parquet"), 400, 10);
        let reshaped = refresh_cube(
            &rebuilt.cube,
            &rebuilt.partials,
            path,
            DataFormat::Parquet,
            &additive_shape(),
            &options(),
        )
        .unwrap()
        .expect("a new version");
        assert!(reshaped.replace_partials);
        assert_eq!(reshaped.cube.shape, additive_shape());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_axis_over_its_cap_is_excluded_and_the_cell_limit_refuses() {
        let directory = directory("caps");
        let location = directory.join("events");
        std::fs::create_dir_all(&location).unwrap();
        write_file(&location.join("a.parquet"), 0, 100);
        let path = location.to_str().unwrap();
        let capped = TableShape::parse(
            &["region".into(), "status:2".into(), "user_id:40".into()],
            &["total:sum".into()],
            None,
        )
        .unwrap();
        let built = build_cube(path, DataFormat::Parquet, table_id(), &capped, &options()).unwrap();
        assert_eq!(
            built.cube.excluded,
            vec![
                ExcludedAxis {
                    column: "status".into(),
                    distinct: 3,
                    cap: 2
                },
                ExcludedAxis {
                    column: "user_id".into(),
                    distinct: 41,
                    cap: 40
                }
            ]
        );
        assert_eq!(
            built
                .cube
                .groupings
                .iter()
                .map(|g| g.axes.clone())
                .collect::<Vec<_>>(),
            vec![vec![], vec![0]]
        );
        assert_eq!(
            built.cube.axes()[0],
            ShapeAxis {
                column: "region".into(),
                cap: kaveon_core::shape::DEFAULT_DIMENSION_CAP,
                grain: None
            }
        );
        let error = build_cube(
            path,
            DataFormat::Parquet,
            table_id(),
            &shape(),
            &CubeBuildOptions {
                memory: None,
                threads: 1,
                max_cells: 10,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("above the limit of 10"), "{error}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
