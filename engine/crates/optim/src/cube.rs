//! Answering a statement from a table's cube: the statements a cube covers
//! are recognised on the optimised logical plan (`match_cube_query`) and
//! answered from its cells (`answer`) with no scan. Exactly this set:
//!
//! - one table, `GROUP BY` a subset of the declared dimensions (none is
//!   the grand total), the time dimension at its grain (the date column
//!   itself at day grain, or `DATE_TRUNC('day' | 'month', ts)` at the
//!   declared grain), or both — at most two axes counting the predicate's;
//! - aggregates `COUNT(*)`, and `SUM`, `COUNT`, `MIN`, `MAX` of a column
//!   declared under that aggregate; `APPROX_COUNT_DISTINCT(col)` — or a
//!   `COUNT(DISTINCT col)` the `approximate` setting lowered to it — of a
//!   column declared `count_distinct`; an exact `COUNT(DISTINCT)` never;
//! - an optional `WHERE` that is a conjunction of `dim = literal` and
//!   `dim IN (literals)` over declared dimensions;
//! - a projection that keeps the keys and aggregates as they are, renamed
//!   at most. `ORDER BY`, `LIMIT`, `HAVING`, expressions over aggregates,
//!   joins and everything else take the row path.
//!
//! The cube must be current for the statement's pinned version; the
//! caller establishes that.

use arrow::array::{ArrayRef, RecordBatch};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use kaveon_core::{
    BinaryOp, CellMeasure, CubeCell, Expr, KaveonError, MeasureAggregate, Result, ScalarValue,
    ShapeAxis, StatValue, TableCube, TableShape, TimeGrain, cube::compare_keys,
};
use kaveon_sql::logical_plan::{AggregateExpr, LogicalPlan};
use std::collections::HashMap;
use std::sync::Arc;

/// One output column of a cube-answered statement.
#[derive(Clone, Debug, PartialEq)]
pub enum CubeOutput {
    /// A group key: the axis (index into the shape's axes).
    Key { axis: usize, name: String },
    /// The row count, `COUNT(*)`.
    Rows { name: String },
    /// A declared measure.
    Measure {
        column: String,
        aggregate: MeasureAggregate,
        name: String,
        /// The function as the statement wrote it (`COUNT` for a distinct
        /// count the `approximate` setting lowered to a sketch).
        written: &'static str,
    },
}

impl CubeOutput {
    pub fn name(&self) -> &str {
        match self {
            Self::Key { name, .. } | Self::Rows { name } | Self::Measure { name, .. } => name,
        }
    }
}

/// A statement the cube covers, as matched on the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeQuery {
    /// The scanned table, as the plan names it.
    pub table: String,
    pub outputs: Vec<CubeOutput>,
    /// The axes grouped by, in `GROUP BY` order.
    pub group_axes: Vec<usize>,
    /// Equality and `IN` predicates: an axis and the literals it may equal,
    /// each coerced for the column's type; a conjunction.
    pub predicates: Vec<(usize, Vec<ScalarValue>)>,
}

impl CubeQuery {
    /// The axes the answer needs a grouping over: the group axes and the
    /// predicate axes, each once.
    pub fn needed_axes(&self) -> Vec<usize> {
        let mut axes = self.group_axes.clone();
        for (axis, _) in &self.predicates {
            if !axes.contains(axis) {
                axes.push(*axis);
            }
        }
        axes
    }

    /// The distinct-count outputs, `(written function, column)`, each an
    /// estimate.
    pub fn approximate_outputs(&self) -> Vec<(&'static str, String)> {
        self.outputs
            .iter()
            .filter_map(|output| match output {
                CubeOutput::Measure {
                    column,
                    aggregate: MeasureAggregate::CountDistinct,
                    written,
                    ..
                } => Some((*written, column.clone())),
                _ => None,
            })
            .collect()
    }
}

/// The table a plan would ask a cube for, when it has the shape of a
/// cube-answerable statement (before the shape and the cube are known):
/// the one scan under an aggregate.
pub fn cube_candidate_table(plan: &LogicalPlan) -> Option<&str> {
    let aggregate = match plan {
        LogicalPlan::Project { input, .. } => input.as_ref(),
        other => other,
    };
    let LogicalPlan::Aggregate { input, .. } = aggregate else {
        return None;
    };
    let mut node = input.as_ref();
    loop {
        match node {
            LogicalPlan::Project { input, .. } | LogicalPlan::Filter { input, .. } => {
                node = input.as_ref();
            }
            LogicalPlan::Scan { table, .. } => return Some(table),
            _ => return None,
        }
    }
}

/// Recognise a cube-answerable statement over a table with `shape` and
/// `schema`; `None` for anything the cube does not cover.
pub fn match_cube_query(
    plan: &LogicalPlan,
    shape: &TableShape,
    schema: &SchemaRef,
) -> Option<CubeQuery> {
    if shape.is_empty() {
        return None;
    }
    let axes = shape.axes();
    let (projection, aggregate) = match plan {
        LogicalPlan::Project { input, columns } => (Some(columns), input.as_ref()),
        other => (None, other),
    };
    let LogicalPlan::Aggregate {
        input,
        group_by,
        aggregates,
    } = aggregate
    else {
        return None;
    };
    // Below the aggregate: an optional projection of aliases (complex
    // group keys and aggregate arguments), an optional filter, the scan.
    let mut aliases: HashMap<&str, &Expr> = HashMap::new();
    let mut predicate: Option<&Expr> = None;
    let mut node = input.as_ref();
    let table = loop {
        match node {
            LogicalPlan::Project { input, columns } => {
                if !aliases.is_empty() {
                    return None;
                }
                for column in columns {
                    match column {
                        Expr::Alias { expr, name } => {
                            aliases.insert(name.as_str(), expr.as_ref());
                        }
                        Expr::Column(name) => {
                            aliases.insert(name.as_str(), column);
                        }
                        _ => return None,
                    }
                }
                node = input.as_ref();
            }
            LogicalPlan::Filter {
                input,
                predicate: p,
            } => {
                if predicate.is_some() {
                    return None;
                }
                predicate = Some(p);
                node = input.as_ref();
            }
            LogicalPlan::Scan { table, .. } => break table.clone(),
            _ => return None,
        }
    };
    let resolve = |expr: &Expr| -> Expr {
        match expr {
            Expr::Column(name) => aliases
                .get(name.as_str())
                .map(|e| (*e).clone())
                .unwrap_or_else(|| expr.clone()),
            other => other.clone(),
        }
    };
    let column_type = |name: &str| schema.field_with_name(name).ok().map(Field::data_type);
    // Group keys.
    let mut group_axes = Vec::with_capacity(group_by.len());
    let mut key_names: Vec<String> = Vec::with_capacity(group_by.len());
    for key in group_by {
        let Expr::Column(key_name) = key else {
            return None;
        };
        let axis = match resolve(key) {
            Expr::Column(column) => {
                if column.contains('.') {
                    return None;
                }
                axes.iter().position(|axis| {
                    axis.column == column
                        && match axis.grain {
                            None => true,
                            // The time column itself is at its grain only
                            // for a date at day grain.
                            Some(grain) => {
                                grain == TimeGrain::Day
                                    && matches!(column_type(&column), Some(DataType::Date32))
                            }
                        }
                })?
            }
            Expr::Function { name, args } if name.eq_ignore_ascii_case("DATE_TRUNC") => {
                let [
                    Expr::Literal(ScalarValue::Utf8(grain)),
                    Expr::Column(column),
                ] = args.as_slice()
                else {
                    return None;
                };
                let grain = TimeGrain::parse(grain)?;
                axes.iter().position(|axis| {
                    axis.column == *column
                        && axis.grain == Some(grain)
                        && matches!(
                            column_type(column),
                            Some(DataType::Timestamp(TimeUnit::Microsecond, _))
                        )
                })?
            }
            _ => return None,
        };
        if group_axes.contains(&axis) {
            return None;
        }
        group_axes.push(axis);
        key_names.push(key_name.clone());
    }
    // Aggregates.
    let mut measures = Vec::with_capacity(aggregates.len());
    for aggregate in aggregates {
        let argument = resolve(aggregate.argument());
        let measure = match (aggregate, &argument) {
            (
                AggregateExpr::Count {
                    distinct: false, ..
                },
                Expr::Star,
            ) => MatchedMeasure::Rows,
            (
                AggregateExpr::Count {
                    distinct: false, ..
                },
                Expr::Column(column),
            ) => MatchedMeasure::Declared(column.clone(), MeasureAggregate::Count),
            (
                AggregateExpr::Sum {
                    distinct: false, ..
                },
                Expr::Column(column),
            ) => MatchedMeasure::Declared(column.clone(), MeasureAggregate::Sum),
            (AggregateExpr::Min(_), Expr::Column(column)) => {
                MatchedMeasure::Declared(column.clone(), MeasureAggregate::Min)
            }
            (AggregateExpr::Max(_), Expr::Column(column)) => {
                MatchedMeasure::Declared(column.clone(), MeasureAggregate::Max)
            }
            (AggregateExpr::ApproxDistinct { .. }, Expr::Column(column)) => {
                MatchedMeasure::Declared(column.clone(), MeasureAggregate::CountDistinct)
            }
            _ => return None,
        };
        if let MatchedMeasure::Declared(column, kind) = &measure {
            if column.contains('.') {
                return None;
            }
            let declared = shape
                .measures
                .iter()
                .any(|measure| &measure.column == column && measure.aggregates.contains(kind));
            if !declared {
                return None;
            }
        }
        measures.push((measure, aggregate.written_name(), aggregate.output_name()));
    }
    // The predicate: a conjunction of equalities and IN lists over
    // dimensions.
    let mut predicates = Vec::new();
    if let Some(predicate) = predicate {
        let mut terms = Vec::new();
        conjuncts(predicate, &mut terms);
        for term in terms {
            let (column, values) = match term {
                Expr::BinaryOp {
                    left,
                    op: BinaryOp::Eq,
                    right,
                } => match (resolve(left), resolve(right)) {
                    (Expr::Column(column), Expr::Literal(value))
                    | (Expr::Literal(value), Expr::Column(column)) => (column, vec![value]),
                    _ => return None,
                },
                Expr::InList {
                    expr,
                    list,
                    negated: false,
                } => {
                    let Expr::Column(column) = resolve(expr) else {
                        return None;
                    };
                    let values = list
                        .iter()
                        .map(|item| match item {
                            Expr::Literal(value) => Some(value.clone()),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?;
                    (column, values)
                }
                _ => return None,
            };
            if column.contains('.') || values.iter().any(|v| matches!(v, ScalarValue::Null)) {
                return None;
            }
            let axis = axes
                .iter()
                .position(|axis| axis.column == column && axis.grain.is_none())?;
            let data_type = column_type(&column)?;
            let values = values
                .into_iter()
                .map(|value| value.coerced_for(data_type))
                .collect();
            predicates.push((axis, values));
        }
    }
    // The projection: keys and aggregates as they are, renamed at most.
    let mut outputs = Vec::new();
    match projection {
        None => {
            for (axis, name) in group_axes.iter().zip(&key_names) {
                outputs.push(CubeOutput::Key {
                    axis: *axis,
                    name: name.clone(),
                });
            }
            for (measure, written, name) in &measures {
                outputs.push(measure.output(name.clone(), written));
            }
        }
        Some(columns) => {
            for column in columns {
                let (expr, alias) = match column {
                    Expr::Alias { expr, name } => (expr.as_ref(), Some(name.clone())),
                    other => (other, None),
                };
                let output = match expr {
                    Expr::Column(name) => {
                        if let Some(index) = key_names.iter().position(|key| key == name) {
                            CubeOutput::Key {
                                axis: group_axes[index],
                                name: alias.unwrap_or_else(|| name.clone()),
                            }
                        } else if let Some((measure, written, _)) =
                            measures.iter().find(|(_, _, output)| output == name)
                        {
                            measure.output(alias.unwrap_or_else(|| name.clone()), written)
                        } else {
                            return None;
                        }
                    }
                    Expr::Function { name, args } => {
                        let (index, (measure, written, output_name)) = aggregates
                            .iter()
                            .zip(&measures)
                            .enumerate()
                            .map(|(index, (aggregate, measure))| (index, aggregate, measure))
                            .find(|(_, aggregate, _)| {
                                name.eq_ignore_ascii_case(aggregate.written_name())
                                    && *args == aggregate.arguments()
                            })
                            .map(|(index, _, measure)| (index, measure))?;
                        let _ = index;
                        measure.output(alias.unwrap_or_else(|| output_name.clone()), written)
                    }
                    _ => return None,
                };
                outputs.push(output);
            }
        }
    }
    let query = CubeQuery {
        table,
        outputs,
        group_axes,
        predicates,
    };
    // The cube holds groupings of at most two axes.
    (query.needed_axes().len() <= 2).then_some(query)
}

enum MatchedMeasure {
    Rows,
    Declared(String, MeasureAggregate),
}

impl MatchedMeasure {
    fn output(&self, name: String, written: &'static str) -> CubeOutput {
        match self {
            Self::Rows => CubeOutput::Rows { name },
            Self::Declared(column, aggregate) => CubeOutput::Measure {
                column: column.clone(),
                aggregate: *aggregate,
                name,
                written,
            },
        }
    }
}

fn conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::And(left, right) => {
            conjuncts(left, out);
            conjuncts(right, out);
        }
        other => out.push(other),
    }
}

/// The answer to `query` from `cube`: the columns and rows as a batch, or
/// `None` when the cube holds no grouping the statement needs (an axis
/// excluded for its cap, a pair beyond the pair limit, a measure slot the
/// cube was built without).
pub fn answer(
    query: &CubeQuery,
    cube: &TableCube,
    schema: &SchemaRef,
) -> Result<Option<RecordBatch>> {
    let needed = query.needed_axes();
    if needed.len() > 2 {
        return Ok(None);
    }
    let Some(grouping) = cube.grouping(&needed) else {
        return Ok(None);
    };
    let axes = cube.axes();
    // Every measure the statement wants must be a slot of the cube.
    let mut slots = Vec::with_capacity(query.outputs.len());
    for output in &query.outputs {
        slots.push(match output {
            CubeOutput::Measure {
                column, aggregate, ..
            } => match cube.slot(column, *aggregate) {
                Some(slot) => Some(slot),
                None => return Ok(None),
            },
            _ => None,
        });
    }
    let position = |axis: usize| grouping.axes.iter().position(|a| *a == axis);
    let predicate_positions: Vec<(usize, &[ScalarValue])> = query
        .predicates
        .iter()
        .map(|(axis, values)| (position(*axis).expect("needed axis"), values.as_slice()))
        .collect();
    let group_positions: Vec<usize> = query
        .group_axes
        .iter()
        .map(|axis| position(*axis).expect("needed axis"))
        .collect();
    // Filter the cells, then roll up along the predicate-only axes.
    let mut rolled: Vec<CubeCell> = Vec::new();
    let slot_kinds: Vec<MeasureAggregate> = cube.slots.iter().map(|(_, kind)| *kind).collect();
    for cell in &grouping.cells {
        let admitted = predicate_positions.iter().all(|(position, values)| {
            cell.key[*position]
                .as_ref()
                .and_then(StatValue::to_scalar)
                .is_some_and(|key| values.iter().any(|value| scalar_eq(&key, value)))
        });
        if !admitted {
            continue;
        }
        let key: Vec<Option<StatValue>> = group_positions
            .iter()
            .map(|position| cell.key[*position].clone())
            .collect();
        match rolled.binary_search_by(|probe| compare_keys(&probe.key, &key)) {
            Ok(index) => rolled[index].fold_in(cell)?,
            Err(index) => {
                let mut fresh = CubeCell::empty(key, &slot_kinds);
                fresh.fold_in(cell)?;
                rolled.insert(index, fresh);
            }
        }
    }
    if query.group_axes.is_empty() && rolled.is_empty() {
        // A grand total over no rows is one row of empty aggregates.
        rolled.push(CubeCell::empty(Vec::new(), &slot_kinds));
    }
    // The columns.
    let mut fields = Vec::with_capacity(query.outputs.len());
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(query.outputs.len());
    for (output, slot) in query.outputs.iter().zip(&slots) {
        let (data_type, values): (DataType, Vec<Option<StatValue>>) = match output {
            CubeOutput::Key { axis, name: _ } => {
                let column_type = axis_type(&axes[*axis], schema)?;
                // The rolled-up keys are in `GROUP BY` order.
                let position = query
                    .group_axes
                    .iter()
                    .position(|a| a == axis)
                    .expect("group axis");
                (
                    column_type,
                    rolled
                        .iter()
                        .map(|cell| cell.key[position].clone())
                        .collect(),
                )
            }
            CubeOutput::Rows { .. } => (
                DataType::UInt64,
                rolled
                    .iter()
                    .map(|cell| Some(StatValue::Int(i128::from(cell.rows))))
                    .collect(),
            ),
            CubeOutput::Measure {
                column, aggregate, ..
            } => {
                let slot = slot.expect("checked above");
                let column_type = schema
                    .field_with_name(column)
                    .map(|field| logical_type(field.data_type()).clone())
                    .map_err(|_| {
                        KaveonError::Execution(format!("column '{column}' is not in the schema"))
                    })?;
                let data_type = measure_output_type(*aggregate, &column_type);
                let values = rolled
                    .iter()
                    .map(|cell| measure_value(&cell.measures[slot]))
                    .collect();
                (data_type, values)
            }
        };
        columns.push(array_from_values(&values, &data_type)?);
        fields.push(Field::new(output.name(), data_type, true));
    }
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)?;
    Ok(Some(batch))
}

fn scalar_eq(left: &ScalarValue, right: &ScalarValue) -> bool {
    match (left, right) {
        (ScalarValue::Bool(a), ScalarValue::Bool(b)) => a == b,
        (ScalarValue::Int64(a), ScalarValue::Int64(b)) => a == b,
        (ScalarValue::Float64(a), ScalarValue::Float64(b)) => a == b,
        (ScalarValue::Utf8(a), ScalarValue::Utf8(b)) => a == b,
        _ => false,
    }
}

fn logical_type(data_type: &DataType) -> &DataType {
    match data_type {
        DataType::Dictionary(_, values) => values.as_ref(),
        other => other,
    }
}

/// The type of an axis's key column: the column's own type (a truncated
/// timestamp keeps its unit and zone).
fn axis_type(axis: &ShapeAxis, schema: &SchemaRef) -> Result<DataType> {
    schema
        .field_with_name(&axis.column)
        .map(|field| logical_type(field.data_type()).clone())
        .map_err(|_| {
            KaveonError::Execution(format!("column '{}' is not in the schema", axis.column))
        })
}

/// The output type of an aggregate over a column of `input`, as the
/// executor types it (`kaveon_exec::aggregate::aggregate_output_types`):
/// counts unsigned; integer sums as `Int64` (`UInt64` stays), decimal
/// sums at precision 38, other sums as doubles; bounds in the column's
/// type for 32- and 64-bit integers, text, dates and decimals, doubles
/// otherwise.
pub fn measure_output_type(aggregate: MeasureAggregate, input: &DataType) -> DataType {
    match aggregate {
        MeasureAggregate::Count | MeasureAggregate::CountDistinct => DataType::UInt64,
        MeasureAggregate::Sum => match input {
            DataType::Int32 | DataType::Int64 => DataType::Int64,
            DataType::UInt64 => DataType::UInt64,
            DataType::Decimal128(_, scale) => DataType::Decimal128(38, *scale),
            _ => DataType::Float64,
        },
        MeasureAggregate::Min | MeasureAggregate::Max => match input {
            DataType::Int32
            | DataType::Int64
            | DataType::UInt64
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Date32
            | DataType::Decimal128(_, _) => input.clone(),
            _ => DataType::Float64,
        },
    }
}

fn measure_value(measure: &CellMeasure) -> Option<StatValue> {
    match measure {
        CellMeasure::Sum(value) | CellMeasure::Min(value) | CellMeasure::Max(value) => {
            value.clone()
        }
        CellMeasure::Count(count) => Some(StatValue::Int(i128::from(*count))),
        CellMeasure::Distinct(sketch) => Some(StatValue::Int(i128::from(sketch.estimate()))),
    }
}

/// An array of `data_type` from logical values; a value of another kind
/// is converted where the number line allows (an integer into a double)
/// and an error otherwise.
fn array_from_values(values: &[Option<StatValue>], data_type: &DataType) -> Result<ArrayRef> {
    use arrow::array::*;
    let mismatch = |value: &StatValue| {
        KaveonError::Execution(format!("cube value {value:?} is not a {data_type}"))
    };
    macro_rules! integers {
        ($array:ty, $cast:ty) => {{
            let mut out: Vec<Option<$cast>> = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Int(v)) => {
                        Some(<$cast>::try_from(*v).map_err(|_| mismatch(&StatValue::Int(*v)))?)
                    }
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(<$array>::from(out)) as ArrayRef)
        }};
    }
    match data_type {
        DataType::Boolean => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Bool(v)) => Some(*v),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(BooleanArray::from(out)))
        }
        DataType::Int8 => integers!(Int8Array, i8),
        DataType::Int16 => integers!(Int16Array, i16),
        DataType::Int32 => integers!(Int32Array, i32),
        DataType::Int64 => integers!(Int64Array, i64),
        DataType::UInt8 => integers!(UInt8Array, u8),
        DataType::UInt16 => integers!(UInt16Array, u16),
        DataType::UInt32 => integers!(UInt32Array, u32),
        DataType::UInt64 => integers!(UInt64Array, u64),
        DataType::Float64 => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Float(v)) => Some(*v),
                    Some(StatValue::Int(v)) => Some(*v as f64),
                    Some(other) => other.to_f64().map(Some).unwrap_or_else(|| Some(f64::NAN)),
                });
            }
            Ok(Arc::new(Float64Array::from(out)))
        }
        DataType::Utf8 => {
            let mut out: Vec<Option<&str>> = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Text(v)) => Some(v.as_str()),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(StringArray::from(out)))
        }
        DataType::LargeUtf8 => {
            let mut out: Vec<Option<&str>> = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Text(v)) => Some(v.as_str()),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(LargeStringArray::from(out)))
        }
        DataType::Date32 => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Date(v)) => Some(*v),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(Date32Array::from(out)))
        }
        DataType::Timestamp(TimeUnit::Microsecond, zone) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Timestamp {
                        value,
                        unit: TimeUnit::Microsecond,
                        ..
                    }) => Some(*value),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(
                TimestampMicrosecondArray::from(out).with_timezone_opt(zone.clone()),
            ))
        }
        DataType::Decimal128(precision, scale) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(match value {
                    None => None,
                    Some(StatValue::Decimal {
                        unscaled,
                        scale: value_scale,
                    }) if value_scale == scale => Some(*unscaled),
                    Some(other) => return Err(mismatch(other)),
                });
            }
            Ok(Arc::new(
                Decimal128Array::from(out).with_precision_and_scale(*precision, *scale)?,
            ))
        }
        other => Err(KaveonError::Execution(format!(
            "a cube cannot answer a {other} column"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaveon_core::{
        AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
        TableMeta,
    };
    use kaveon_sql::logical_plan::{
        sql_to_logical_plan_for_binder, sql_to_logical_plan_for_binder_approximate,
    };
    use std::path::PathBuf;

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("region", DataType::Utf8, true),
            Field::new("status", DataType::Utf8, true),
            Field::new("total", DataType::Int64, true),
            Field::new("score", DataType::Float64, true),
            Field::new("user_id", DataType::Int64, true),
            Field::new("day", DataType::Date32, true),
            Field::new("at", DataType::Timestamp(TimeUnit::Microsecond, None), true),
        ]))
    }

    fn catalog() -> CatalogManager {
        let mut memory = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: PathBuf::from("."),
            },
        )
        .with_schema("s");
        memory
            .register_table(
                "s",
                TableMeta {
                    name: "t".into(),
                    arrow_schema: schema(),
                    location: "t.parquet".into(),
                    access: AccessPattern::Shortcut,
                    format: DataFormat::Parquet,
                },
            )
            .unwrap();
        let mut manager = CatalogManager::new("lake", "s");
        manager.register_catalog(Box::new(memory));
        manager
    }

    fn qualify(plan: &mut LogicalPlan) {
        if let LogicalPlan::Scan { table, .. } = plan {
            *table = format!("lake.s.{table}");
        }
        match plan {
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. } => qualify(input),
            _ => {}
        }
    }

    fn optimized(sql: &str, approximate: bool) -> LogicalPlan {
        let mut plan = if approximate {
            sql_to_logical_plan_for_binder_approximate(sql)
        } else {
            sql_to_logical_plan_for_binder(sql)
        }
        .unwrap();
        qualify(&mut plan);
        let plan = crate::binder::bind(plan, &catalog()).unwrap();
        let plan = crate::rules::push_filter_down(plan);
        crate::rules::push_projection_down(plan)
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

    fn matched(sql: &str) -> Option<CubeQuery> {
        match_cube_query(&optimized(sql, false), &shape(), &schema())
    }

    #[test]
    fn covered_statements_match_with_their_axes_measures_predicates_and_names() {
        let total = matched("SELECT COUNT(*), SUM(total) AS s FROM t").unwrap();
        assert_eq!(total.group_axes, Vec::<usize>::new());
        assert!(total.predicates.is_empty());
        assert_eq!(
            total.outputs,
            vec![
                CubeOutput::Rows {
                    name: "count_*".into()
                },
                CubeOutput::Measure {
                    column: "total".into(),
                    aggregate: MeasureAggregate::Sum,
                    name: "s".into(),
                    written: "SUM",
                }
            ]
        );
        let one = matched("SELECT region, MIN(total), MAX(total) FROM t GROUP BY region").unwrap();
        assert_eq!(one.group_axes, vec![0]);
        assert_eq!(
            one.outputs[0],
            CubeOutput::Key {
                axis: 0,
                name: "region".into()
            }
        );
        assert_eq!(one.outputs[1].name(), "min_total");
        let two = matched(
            "SELECT status AS st, region, COUNT(total) FROM t WHERE region IN ('EU', 'US') GROUP BY status, region",
        )
        .unwrap();
        assert_eq!(two.group_axes, vec![1, 0]);
        assert_eq!(
            two.predicates,
            vec![(
                0,
                vec![
                    ScalarValue::Utf8("EU".into()),
                    ScalarValue::Utf8("US".into())
                ]
            )]
        );
        assert_eq!(two.needed_axes(), vec![1, 0]);
        assert_eq!(two.outputs[0].name(), "st");
        let day = matched("SELECT day, SUM(score) FROM t GROUP BY day").unwrap();
        assert_eq!(day.group_axes, vec![2]);
        let filtered =
            matched("SELECT SUM(total) FROM t WHERE region = 'EU' AND status = 'open'").unwrap();
        assert_eq!(filtered.needed_axes(), vec![0, 1]);
        let approximate = match_cube_query(
            &optimized(
                "SELECT region, COUNT(DISTINCT user_id) AS users FROM t GROUP BY region",
                true,
            ),
            &shape(),
            &schema(),
        )
        .unwrap();
        assert_eq!(
            approximate.outputs[1],
            CubeOutput::Measure {
                column: "user_id".into(),
                aggregate: MeasureAggregate::CountDistinct,
                name: "users".into(),
                written: "COUNT",
            }
        );
        assert_eq!(
            approximate.approximate_outputs(),
            vec![("COUNT", "user_id".to_owned())]
        );
        let explicit =
            matched("SELECT APPROX_COUNT_DISTINCT(user_id) FROM t WHERE status = 'open'").unwrap();
        assert_eq!(explicit.approximate_outputs()[0].0, "APPROX_COUNT_DISTINCT");
        // A timestamp time column at its grain.
        let ts_shape = TableShape::parse(
            &["region:10".into()],
            &["total:sum".into()],
            Some("at:month"),
        )
        .unwrap();
        let month = match_cube_query(
            &optimized(
                "SELECT date_trunc('month', at) AS m, SUM(total) FROM t GROUP BY date_trunc('month', at)",
                false,
            ),
            &ts_shape,
            &schema(),
        )
        .unwrap();
        assert_eq!(month.group_axes, vec![1]);
        assert_eq!(
            month.outputs[0],
            CubeOutput::Key {
                axis: 1,
                name: "m".into()
            }
        );
        assert_eq!(
            cube_candidate_table(&optimized(
                "SELECT COUNT(*) FROM t WHERE region = 'EU'",
                false
            )),
            Some("lake.s.t")
        );
    }

    #[test]
    fn uncovered_statements_take_the_row_path() {
        for sql in [
            // An exact distinct count, never.
            "SELECT COUNT(DISTINCT user_id) FROM t",
            // An undeclared measure or aggregate.
            "SELECT AVG(total) FROM t",
            "SELECT SUM(user_id) FROM t",
            "SELECT MIN(score) FROM t",
            // A group by a non-dimension.
            "SELECT user_id, COUNT(*) FROM t GROUP BY user_id",
            // A predicate on a non-dimension, a range, a disjunction, a null.
            "SELECT COUNT(*) FROM t WHERE total > 5",
            "SELECT COUNT(*) FROM t WHERE region > 'A'",
            "SELECT COUNT(*) FROM t WHERE region = 'EU' OR status = 'open'",
            "SELECT COUNT(*) FROM t WHERE region IS NULL",
            "SELECT COUNT(*) FROM t WHERE day = DATE '2024-01-01'",
            // The time column at another grain.
            "SELECT date_trunc('month', at), COUNT(*) FROM t GROUP BY date_trunc('month', at)",
            // Expressions over aggregates, ordering, limits, having.
            "SELECT SUM(total) + 1 FROM t",
            "SELECT region, SUM(total) FROM t GROUP BY region ORDER BY region",
            "SELECT region, SUM(total) FROM t GROUP BY region LIMIT 3",
            "SELECT region FROM t GROUP BY region HAVING SUM(total) > 1",
            // Three axes, grouped or counting the predicate's.
            "SELECT region, status, day, COUNT(*) FROM t GROUP BY region, status, day",
            "SELECT region, day, COUNT(*) FROM t WHERE status = 'open' GROUP BY region, day",
        ] {
            assert!(matched(sql).is_none(), "{sql}");
        }
    }

    /// A hand-built cube over region × status with sums, counts and a
    /// distinct count.
    fn cube() -> TableCube {
        use kaveon_core::{CubeGrouping, HllSketch, SourceVersion, SourceVersionKind, TableId};
        let shape = shape();
        let slots = shape.measure_slots();
        let kinds: Vec<MeasureAggregate> = slots.iter().map(|(_, k)| *k).collect();
        let text = |v: &str| Some(StatValue::Text(v.into()));
        // (region, status, total values, users)
        type Fact = (
            Option<&'static str>,
            &'static str,
            &'static [i64],
            &'static [i64],
        );
        let facts: [Fact; 5] = [
            (Some("EU"), "open", &[1, 2], &[1, 2]),
            (Some("EU"), "closed", &[10], &[1]),
            (Some("US"), "open", &[5, 6, 7], &[3, 4, 3]),
            (Some("US"), "hold", &[], &[]),
            (None, "open", &[100], &[9]),
        ];
        let make_cell = |key: Vec<Option<StatValue>>, rows: &[(&[i64], &[i64])]| {
            let mut cell = CubeCell::empty(key, &kinds);
            let mut sketch = HllSketch::default_precision();
            for (totals, users) in rows {
                cell.rows += totals.len().max(1) as u64;
                for total in *totals {
                    for slot in 0..4 {
                        cell.measures[slot]
                            .fold_value(&StatValue::Int(i128::from(*total)))
                            .unwrap();
                    }
                    cell.measures[4]
                        .fold_value(&StatValue::Float(*total as f64 / 2.0))
                        .unwrap();
                }
                for user in *users {
                    sketch.insert_text(&user.to_string());
                }
            }
            cell.measures[5] = CellMeasure::Distinct(sketch);
            cell
        };
        let mut groupings = Vec::new();
        // Grand total.
        groupings.push(CubeGrouping {
            axes: vec![],
            cells: vec![make_cell(
                vec![],
                &facts
                    .iter()
                    .map(|(_, _, t, u)| (*t, *u))
                    .collect::<Vec<_>>(),
            )],
        });
        // By region.
        let mut by_region = Vec::new();
        for region in [None, Some("EU"), Some("US")] {
            let rows: Vec<_> = facts
                .iter()
                .filter(|(r, _, _, _)| *r == region)
                .map(|(_, _, t, u)| (*t, *u))
                .collect();
            by_region.push(make_cell(vec![region.and_then(text)], &rows));
        }
        groupings.push(CubeGrouping {
            axes: vec![0],
            cells: by_region,
        });
        // By status.
        let mut by_status = Vec::new();
        for status in ["closed", "hold", "open"] {
            let rows: Vec<_> = facts
                .iter()
                .filter(|(_, s, _, _)| *s == status)
                .map(|(_, _, t, u)| (*t, *u))
                .collect();
            by_status.push(make_cell(vec![text(status)], &rows));
        }
        groupings.push(CubeGrouping {
            axes: vec![1],
            cells: by_status,
        });
        // Region × status.
        groupings.push(CubeGrouping {
            axes: vec![0, 1],
            cells: facts
                .iter()
                .map(|(r, s, t, u)| make_cell(vec![r.and_then(text), text(s)], &[(*t, *u)]))
                .collect(),
        });
        TableCube {
            version: kaveon_core::TABLE_CUBE_VERSION,
            table_id: TableId::new("table:lake:s:t").unwrap(),
            source_version: SourceVersion {
                identity_sha256: "v1".into(),
                kind: SourceVersionKind::File,
            },
            computed_at_ms: 0,
            slots,
            shape,
            groupings,
            excluded: Vec::new(),
            files: vec!["t.parquet".into()],
            per_file_complete: true,
        }
    }

    fn rows(batch: &RecordBatch) -> Vec<Vec<String>> {
        use arrow::array::Array;
        let columns: Vec<_> = batch
            .columns()
            .iter()
            .map(|column| arrow::compute::cast(column, &DataType::Utf8).unwrap())
            .collect();
        (0..batch.num_rows())
            .map(|row| {
                columns
                    .iter()
                    .map(|column| {
                        let column = column
                            .as_any()
                            .downcast_ref::<arrow::array::StringArray>()
                            .unwrap();
                        if column.is_null(row) {
                            "NULL".to_owned()
                        } else {
                            column.value(row).to_owned()
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn answers_filter_roll_up_and_type_their_columns() {
        let cube = cube();
        let answered = |sql: &str| -> RecordBatch {
            let query = matched(sql).unwrap_or_else(|| panic!("{sql} matches"));
            answer(&query, &cube, &schema())
                .unwrap()
                .unwrap_or_else(|| panic!("{sql} is answered"))
        };
        let total =
            answered("SELECT COUNT(*), SUM(total), MIN(total), MAX(total), COUNT(total) FROM t");
        assert_eq!(
            total
                .schema()
                .fields()
                .iter()
                .map(|f| f.data_type().clone())
                .collect::<Vec<_>>(),
            vec![
                DataType::UInt64,
                DataType::Int64,
                DataType::Int64,
                DataType::Int64,
                DataType::UInt64
            ]
        );
        assert_eq!(rows(&total), vec![vec!["8", "131", "1", "100", "7"]]);
        // A one-dimension breakdown, null key included, keys ordered.
        let by_region = answered("SELECT region, SUM(total) AS s FROM t GROUP BY region");
        assert_eq!(
            rows(&by_region),
            vec![vec!["NULL", "100"], vec!["EU", "13"], vec!["US", "18"]]
        );
        // A predicate on a dimension not grouped: cells roll up along it.
        let open_or_hold = answered(
            "SELECT region, COUNT(*) AS n, SUM(score) AS sc FROM t WHERE status IN ('open', 'hold') GROUP BY region",
        );
        assert_eq!(
            open_or_hold.schema().field(2).data_type(),
            &DataType::Float64
        );
        assert_eq!(
            rows(&open_or_hold),
            vec![
                vec!["NULL", "1", "50.0"],
                vec!["EU", "2", "1.5"],
                vec!["US", "4", "9.0"]
            ]
        );
        // A predicate that admits no cell: a grand total of empties, a
        // breakdown of no rows.
        assert_eq!(
            rows(&answered(
                "SELECT COUNT(*), SUM(total) FROM t WHERE region = 'APAC'"
            )),
            vec![vec!["0", "NULL"]]
        );
        assert_eq!(
            answered("SELECT region, COUNT(*) FROM t WHERE region = 'APAC' GROUP BY region")
                .num_rows(),
            0
        );
        // The distinct count from the merged sketches.
        let users = match_cube_query(
            &optimized(
                "SELECT COUNT(DISTINCT user_id) AS users FROM t WHERE status = 'open'",
                true,
            ),
            &shape(),
            &schema(),
        )
        .unwrap();
        let users = answer(&users, &cube, &schema()).unwrap().unwrap();
        assert_eq!(rows(&users), vec![vec!["5"]]);
        // Two dimensions, projected in the statement's order.
        let two = answered("SELECT status, region, MAX(total) FROM t GROUP BY region, status");
        assert_eq!(rows(&two)[0], vec!["open", "NULL", "100"]);
        assert_eq!(two.num_rows(), 5);
        // A key whose place in the grouping differs from its place in the
        // statement: grouped by status, filtered by region.
        let status_in_eu =
            answered("SELECT status, SUM(total) FROM t WHERE region = 'EU' GROUP BY status");
        assert_eq!(
            rows(&status_in_eu),
            vec![vec!["closed", "10"], vec!["open", "3"]]
        );
        // A grouping the cube does not hold stands aside.
        let by_day = matched("SELECT day, COUNT(*) FROM t GROUP BY day").unwrap();
        assert!(answer(&by_day, &cube, &schema()).unwrap().is_none());
    }

    #[test]
    fn measure_output_types_mirror_the_executor() {
        assert_eq!(
            measure_output_type(MeasureAggregate::Sum, &DataType::Int32),
            DataType::Int64
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Sum, &DataType::UInt64),
            DataType::UInt64
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Sum, &DataType::Decimal128(10, 2)),
            DataType::Decimal128(38, 2)
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Sum, &DataType::Int16),
            DataType::Float64
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Min, &DataType::Int32),
            DataType::Int32
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Max, &DataType::Float64),
            DataType::Float64
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Max, &DataType::Utf8),
            DataType::Utf8
        );
        assert_eq!(
            measure_output_type(MeasureAggregate::Count, &DataType::Utf8),
            DataType::UInt64
        );
    }
}
