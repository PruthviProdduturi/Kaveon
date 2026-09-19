//! The cube: what the engine knows about a table's measures at every
//! grouping its declared shape names — the grand total, each axis, the
//! time grain and the low-cardinality pairs — at one source version. Each
//! cell holds the additive measures exactly and the distinct counts as
//! HyperLogLog sketches. A catalog object beside the table's statistics,
//! versioned the same way; maintained incrementally from per-file partials.

use crate::shape::{MeasureAggregate, ShapeAxis, TableShape};
use crate::sketch::HllSketch;
use crate::statistics::{SourceVersion, StatValue};
use crate::{KaveonError, Result, TableId};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// The format version of a stored cube document.
pub const TABLE_CUBE_VERSION: u32 = 1;
/// The bytes a stored cube document may occupy per cell of its limit: a
/// cube of at most N cells is at most [`CUBE_BYTES_ALLOWANCE`] + 256 N
/// bytes encoded (a distinct count's sketch is sparse while its cell holds
/// few values and 3 KiB dense, so a cube of many high-distinct cells meets
/// this bound first).
pub const CUBE_BYTES_PER_CELL: u64 = 256;
/// The bytes a cube document may occupy beyond its cells: the shape, the
/// slots, the file list.
pub const CUBE_BYTES_ALLOWANCE: u64 = 1024 * 1024;
/// The most files whose partials a cube keeps; beyond it a removal
/// rebuilds the cube.
pub const MAX_PER_FILE_CUBES: usize = 10_000;
/// The per-file partials together may hold this many times the cell limit
/// in cells; beyond it they are dropped and a removal rebuilds the cube.
pub const CUBE_PARTIALS_MULTIPLIER: u64 = 8;

/// A measure's value in one cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CellMeasure {
    /// `None` when every value was null.
    Sum(Option<StatValue>),
    /// Non-null values.
    Count(u64),
    Min(Option<StatValue>),
    Max(Option<StatValue>),
    Distinct(HllSketch),
}

impl CellMeasure {
    /// An empty accumulator for the aggregate.
    pub fn empty(aggregate: MeasureAggregate) -> Self {
        match aggregate {
            MeasureAggregate::Sum => Self::Sum(None),
            MeasureAggregate::Count => Self::Count(0),
            MeasureAggregate::Min => Self::Min(None),
            MeasureAggregate::Max => Self::Max(None),
            MeasureAggregate::CountDistinct => Self::Distinct(HllSketch::default_precision()),
        }
    }

    pub const fn aggregate(&self) -> MeasureAggregate {
        match self {
            Self::Sum(_) => MeasureAggregate::Sum,
            Self::Count(_) => MeasureAggregate::Count,
            Self::Min(_) => MeasureAggregate::Min,
            Self::Max(_) => MeasureAggregate::Max,
            Self::Distinct(_) => MeasureAggregate::CountDistinct,
        }
    }

    /// Fold one non-null value in.
    pub fn fold_value(&mut self, value: &StatValue) -> Result<()> {
        match self {
            Self::Sum(sum) => *sum = Some(add_values(sum.take(), value)?),
            Self::Count(count) => *count += 1,
            Self::Min(min) => *min = Some(narrow(min.take(), value, Ordering::Less)),
            Self::Max(max) => *max = Some(narrow(max.take(), value, Ordering::Greater)),
            Self::Distinct(sketch) => sketch.insert_text(&value.to_hash_text()),
        }
        Ok(())
    }

    /// Fold another cell's accumulator of the same aggregate in.
    pub fn fold_in(&mut self, other: &CellMeasure) -> Result<()> {
        match (self, other) {
            (Self::Sum(sum), Self::Sum(theirs)) => {
                if let Some(theirs) = theirs {
                    *sum = Some(add_values(sum.take(), theirs)?);
                }
            }
            (Self::Count(count), Self::Count(theirs)) => *count += theirs,
            (Self::Min(min), Self::Min(theirs)) => {
                if let Some(theirs) = theirs {
                    *min = Some(narrow(min.take(), theirs, Ordering::Less));
                }
            }
            (Self::Max(max), Self::Max(theirs)) => {
                if let Some(theirs) = theirs {
                    *max = Some(narrow(max.take(), theirs, Ordering::Greater));
                }
            }
            (Self::Distinct(sketch), Self::Distinct(theirs)) => sketch.merge(theirs)?,
            (mine, theirs) => {
                return Err(KaveonError::Execution(format!(
                    "cube cell folds {} into {}",
                    theirs.aggregate().name(),
                    mine.aggregate().name()
                )));
            }
        }
        Ok(())
    }
}

fn add_values(current: Option<StatValue>, value: &StatValue) -> Result<StatValue> {
    let Some(current) = current else {
        return Ok(value.clone());
    };
    match (current, value) {
        (StatValue::Int(a), StatValue::Int(b)) => a
            .checked_add(*b)
            .map(StatValue::Int)
            .ok_or_else(|| KaveonError::Execution("cube sum overflows".into())),
        (StatValue::Float(a), StatValue::Float(b)) => Ok(StatValue::Float(a + b)),
        (
            StatValue::Decimal {
                unscaled: a,
                scale: sa,
            },
            StatValue::Decimal {
                unscaled: b,
                scale: sb,
            },
        ) if sa == *sb => a
            .checked_add(*b)
            .map(|unscaled| StatValue::Decimal {
                unscaled,
                scale: sa,
            })
            .ok_or_else(|| KaveonError::Execution("cube sum overflows".into())),
        (current, value) => Err(KaveonError::Execution(format!(
            "cube sum mixes {current:?} and {value:?}"
        ))),
    }
}

/// The bound narrowed by a value: the value when it is `prefer` relative
/// to the bound (or the bound is unknown), else the bound.
fn narrow(current: Option<StatValue>, value: &StatValue, prefer: Ordering) -> StatValue {
    match current {
        None => value.clone(),
        Some(current) => match value.partial_cmp(&current) {
            Some(order) if order == prefer => value.clone(),
            _ => current,
        },
    }
}

/// One cell: the key along the grouping's axes (a null value is `None`),
/// the rows it aggregates, and the measures in slot order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CubeCell {
    pub key: Vec<Option<StatValue>>,
    pub rows: u64,
    pub measures: Vec<CellMeasure>,
}

impl CubeCell {
    pub fn empty(key: Vec<Option<StatValue>>, slots: &[MeasureAggregate]) -> Self {
        Self {
            key,
            rows: 0,
            measures: slots.iter().map(|slot| CellMeasure::empty(*slot)).collect(),
        }
    }

    /// Fold another cell with the same key and slots in.
    pub fn fold_in(&mut self, other: &CubeCell) -> Result<()> {
        if other.measures.len() != self.measures.len() {
            return Err(KaveonError::Execution(format!(
                "cube cell folds {} measures into {}",
                other.measures.len(),
                self.measures.len()
            )));
        }
        self.rows += other.rows;
        for (mine, theirs) in self.measures.iter_mut().zip(&other.measures) {
            mine.fold_in(theirs)?;
        }
        Ok(())
    }
}

/// Keys order within an axis: nulls first, then the values' own order;
/// values of different kinds (which one axis never mixes) keep their
/// place.
pub fn compare_keys(left: &[Option<StatValue>], right: &[Option<StatValue>]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let order = match (left, right) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(left), Some(right)) => left.partial_cmp(right).unwrap_or(Ordering::Equal),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// The cells at one grouping: `axes` index [`TableShape::axes`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CubeGrouping {
    pub axes: Vec<usize>,
    pub cells: Vec<CubeCell>,
}

impl CubeGrouping {
    /// Fold another grouping's cells in (the same axes and slots), the
    /// result sorted by key: one pass over both sides once each is sorted.
    pub fn fold_in(&mut self, other: &CubeGrouping) -> Result<()> {
        let mut mine = std::mem::take(&mut self.cells);
        mine.sort_by(|a, b| compare_keys(&a.key, &b.key));
        let mut theirs: Vec<&CubeCell> = other.cells.iter().collect();
        theirs.sort_by(|a, b| compare_keys(&a.key, &b.key));
        let mut merged = Vec::with_capacity(mine.len() + theirs.len());
        let mut mine = mine.into_iter().peekable();
        let mut theirs = theirs.into_iter().peekable();
        loop {
            match (mine.peek(), theirs.peek()) {
                (Some(a), Some(b)) => match compare_keys(&a.key, &b.key) {
                    Ordering::Less => merged.push(mine.next().expect("peeked")),
                    Ordering::Greater => merged.push(theirs.next().expect("peeked").clone()),
                    Ordering::Equal => {
                        let mut cell = mine.next().expect("peeked");
                        cell.fold_in(theirs.next().expect("peeked"))?;
                        merged.push(cell);
                    }
                },
                (Some(_), None) => merged.push(mine.next().expect("peeked")),
                (None, Some(_)) => merged.push(theirs.next().expect("peeked").clone()),
                (None, None) => break,
            }
        }
        self.cells = merged;
        Ok(())
    }
}

/// An axis left out of the cube because it holds more distinct values
/// than its cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedAxis {
    pub column: String,
    pub distinct: u64,
    pub cap: u64,
}

/// A table's cube at one source version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableCube {
    pub version: u32,
    pub table_id: TableId,
    pub source_version: SourceVersion,
    /// Milliseconds since the epoch.
    pub computed_at_ms: u64,
    /// The shape the cube was built over; a changed declaration rebuilds.
    pub shape: TableShape,
    /// The measure slots, `(column, aggregate)`, in cell order.
    pub slots: Vec<(String, MeasureAggregate)>,
    pub groupings: Vec<CubeGrouping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<ExcludedAxis>,
    /// The files that contributed, by their labels, so a new listing tells
    /// what was added and what removed.
    #[serde(default)]
    pub files: Vec<String>,
    /// Whether every contributing file's partial is on record.
    #[serde(default)]
    pub per_file_complete: bool,
}

impl TableCube {
    pub fn axes(&self) -> Vec<ShapeAxis> {
        self.shape.axes()
    }

    pub fn is_current_for(&self, identity_sha256: &str) -> bool {
        self.source_version.identity_sha256 == identity_sha256
    }

    pub fn cell_count(&self) -> u64 {
        self.groupings
            .iter()
            .map(|grouping| grouping.cells.len() as u64)
            .sum()
    }

    /// The grouping over exactly `axes` (in any order), if the cube holds
    /// it.
    pub fn grouping(&self, axes: &[usize]) -> Option<&CubeGrouping> {
        let mut wanted = axes.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        self.groupings.iter().find(|grouping| {
            let mut have = grouping.axes.clone();
            have.sort_unstable();
            have == wanted
        })
    }

    /// The slot index of `(column, aggregate)`.
    pub fn slot(&self, column: &str, aggregate: MeasureAggregate) -> Option<usize> {
        self.slots
            .iter()
            .position(|(name, kind)| name == column && *kind == aggregate)
    }

    /// The slots a per-file partial carries: the additive ones.
    pub fn additive_slots(&self) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, (_, aggregate))| aggregate.is_additive())
            .map(|(index, _)| index)
            .collect()
    }

    /// Drop every axis whose single-axis grouping holds more distinct keys
    /// than its cap, with every grouping it is part of; each is recorded
    /// in `excluded`.
    pub fn apply_caps(&mut self) {
        let axes = self.axes();
        let mut over = Vec::new();
        for (index, axis) in axes.iter().enumerate() {
            if self.excluded.iter().any(|e| e.column == axis.column) {
                over.push(index);
                continue;
            }
            if let Some(grouping) = self.grouping(&[index])
                && grouping.cells.len() as u64 > axis.cap
            {
                self.excluded.push(ExcludedAxis {
                    column: axis.column.clone(),
                    distinct: grouping.cells.len() as u64,
                    cap: axis.cap,
                });
                over.push(index);
            }
        }
        if !over.is_empty() {
            self.groupings
                .retain(|grouping| !grouping.axes.iter().any(|axis| over.contains(axis)));
        }
    }

    /// The cube held to the limit: at most `max_cells` cells and at most
    /// [`CUBE_BYTES_ALLOWANCE`] + [`CUBE_BYTES_PER_CELL`] × `max_cells`
    /// encoded bytes.
    pub fn check_limit(&self, max_cells: u64) -> Result<Vec<u8>> {
        let cells = self.cell_count();
        if cells > max_cells {
            return Err(KaveonError::Execution(format!(
                "the cube holds {cells} cells, above the limit of {max_cells} (KAVEON_CUBE_MAX_CELLS)"
            )));
        }
        let document = self.to_json_bytes()?;
        let max_bytes =
            CUBE_BYTES_ALLOWANCE.saturating_add(max_cells.saturating_mul(CUBE_BYTES_PER_CELL));
        if document.len() as u64 > max_bytes {
            return Err(KaveonError::Execution(format!(
                "the cube encodes to {} bytes, above the {max_bytes} bytes its cell limit allows ({CUBE_BYTES_ALLOWANCE} plus {CUBE_BYTES_PER_CELL} per cell of KAVEON_CUBE_MAX_CELLS)",
                document.len()
            )));
        }
        Ok(document)
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|error| KaveonError::Execution(format!("cube encode: {error}")))
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|error| KaveonError::Execution(format!("cube decode: {error}")))?;
        if value.version != TABLE_CUBE_VERSION {
            return Err(KaveonError::Execution(format!(
                "cube document version {} is not {TABLE_CUBE_VERSION}",
                value.version
            )));
        }
        Ok(value)
    }
}

/// One file's contribution to the cube: its cells at every grouping, the
/// additive measures only (in [`TableCube::additive_slots`] order), so a
/// removed file's contribution can be taken out again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileCubePartial {
    pub path: String,
    pub rows: u64,
    pub groupings: Vec<CubeGrouping>,
}

impl FileCubePartial {
    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|error| KaveonError::Execution(format!("cube partial encode: {error}")))
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes)
            .map_err(|error| KaveonError::Execution(format!("cube partial decode: {error}")))
    }

    pub fn cell_count(&self) -> u64 {
        self.groupings
            .iter()
            .map(|grouping| grouping.cells.len() as u64)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_fold_values_and_each_other() {
        let mut sum = CellMeasure::empty(MeasureAggregate::Sum);
        sum.fold_value(&StatValue::Int(5)).unwrap();
        sum.fold_value(&StatValue::Int(-2)).unwrap();
        assert_eq!(sum, CellMeasure::Sum(Some(StatValue::Int(3))));
        let mut other = CellMeasure::Sum(Some(StatValue::Int(10)));
        other.fold_in(&sum).unwrap();
        assert_eq!(other, CellMeasure::Sum(Some(StatValue::Int(13))));
        other.fold_in(&CellMeasure::Sum(None)).unwrap();
        assert_eq!(other, CellMeasure::Sum(Some(StatValue::Int(13))));
        assert!(
            CellMeasure::Sum(Some(StatValue::Int(i128::MAX)))
                .fold_value(&StatValue::Int(1))
                .is_err()
        );
        let mut min = CellMeasure::empty(MeasureAggregate::Min);
        let mut max = CellMeasure::empty(MeasureAggregate::Max);
        for value in ["m", "a", "z"] {
            min.fold_value(&StatValue::Text(value.into())).unwrap();
            max.fold_value(&StatValue::Text(value.into())).unwrap();
        }
        assert_eq!(min, CellMeasure::Min(Some(StatValue::Text("a".into()))));
        assert_eq!(max, CellMeasure::Max(Some(StatValue::Text("z".into()))));
        let mut count = CellMeasure::empty(MeasureAggregate::Count);
        count.fold_value(&StatValue::Bool(true)).unwrap();
        count.fold_in(&CellMeasure::Count(4)).unwrap();
        assert_eq!(count, CellMeasure::Count(5));
        let mut distinct = CellMeasure::empty(MeasureAggregate::CountDistinct);
        for value in 0..100 {
            distinct.fold_value(&StatValue::Int(value)).unwrap();
        }
        let CellMeasure::Distinct(sketch) = &distinct else {
            panic!("distinct");
        };
        assert!((90..=110).contains(&sketch.estimate()));
        assert!(count.fold_in(&sum).is_err());
    }

    #[test]
    fn groupings_fold_by_key_and_stay_sorted() {
        let slots = [MeasureAggregate::Sum];
        let cell = |key: Option<i128>, sum: i128, rows: u64| CubeCell {
            key: vec![key.map(StatValue::Int)],
            rows,
            measures: vec![CellMeasure::Sum(Some(StatValue::Int(sum)))],
        };
        let mut grouping = CubeGrouping {
            axes: vec![0],
            cells: vec![cell(Some(3), 30, 1), cell(Some(1), 10, 1)],
        };
        let other = CubeGrouping {
            axes: vec![0],
            cells: vec![cell(None, 5, 2), cell(Some(3), 3, 1), cell(Some(2), 20, 1)],
        };
        grouping.fold_in(&other).unwrap();
        assert_eq!(
            grouping.cells,
            vec![
                cell(None, 5, 2),
                cell(Some(1), 10, 1),
                cell(Some(2), 20, 1),
                cell(Some(3), 33, 2)
            ]
        );
        assert_eq!(CubeCell::empty(vec![None], &slots).measures.len(), 1);
    }

    #[test]
    fn caps_exclude_an_axis_and_the_limit_bounds_cells_and_bytes() {
        let shape =
            TableShape::parse(&["a:2".into(), "b".into()], &["v:sum".into()], None).unwrap();
        let key = |values: &[i128]| values.iter().map(|v| Some(StatValue::Int(*v))).collect();
        let cells = |keys: &[&[i128]]| {
            keys.iter()
                .map(|k| CubeCell {
                    key: key(k),
                    rows: 1,
                    measures: vec![CellMeasure::Sum(Some(StatValue::Int(1)))],
                })
                .collect::<Vec<_>>()
        };
        let mut cube = TableCube {
            version: TABLE_CUBE_VERSION,
            table_id: TableId::new("table:x:y:z").unwrap(),
            source_version: SourceVersion {
                identity_sha256: "abc".into(),
                kind: crate::statistics::SourceVersionKind::File,
            },
            computed_at_ms: 0,
            slots: shape.measure_slots(),
            shape,
            groupings: vec![
                CubeGrouping {
                    axes: vec![],
                    cells: cells(&[&[]]),
                },
                CubeGrouping {
                    axes: vec![0],
                    cells: cells(&[&[1], &[2], &[3]]),
                },
                CubeGrouping {
                    axes: vec![1],
                    cells: cells(&[&[1]]),
                },
                CubeGrouping {
                    axes: vec![0, 1],
                    cells: cells(&[&[1, 1], &[2, 1], &[3, 1]]),
                },
            ],
            excluded: Vec::new(),
            files: vec!["a.parquet".into()],
            per_file_complete: true,
        };
        assert_eq!(cube.cell_count(), 8);
        cube.apply_caps();
        assert_eq!(
            cube.excluded,
            vec![ExcludedAxis {
                column: "a".into(),
                distinct: 3,
                cap: 2
            }]
        );
        assert_eq!(
            cube.groupings
                .iter()
                .map(|g| g.axes.clone())
                .collect::<Vec<_>>(),
            vec![vec![], vec![1]]
        );
        assert!(cube.grouping(&[0]).is_none());
        assert!(cube.grouping(&[1]).is_some());
        assert_eq!(cube.slot("v", MeasureAggregate::Sum), Some(0));
        assert_eq!(cube.slot("v", MeasureAggregate::Count), None);
        let document = cube.check_limit(2).unwrap();
        let decoded = TableCube::from_json_bytes(&document).unwrap();
        assert_eq!(decoded, cube);
        assert!(
            cube.check_limit(1)
                .unwrap_err()
                .to_string()
                .contains("above the limit of 1")
        );
        // Two cells fit the count; a key longer than the allowance does not
        // fit the bytes.
        cube.groupings[1].cells[0].key = vec![Some(StatValue::Text(
            "k".repeat(CUBE_BYTES_ALLOWANCE as usize + 1024),
        ))];
        assert!(
            cube.check_limit(2)
                .unwrap_err()
                .to_string()
                .contains("bytes its cell limit allows")
        );
    }
}
