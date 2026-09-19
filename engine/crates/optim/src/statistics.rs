//! Build-side selection and join placement from what the catalog knows:
//! exact row counts, and — when a table's statistics are on record —
//! bounds, null counts, distinct counts and quantiles that turn a filtered
//! scan into an estimated cardinality. Unsupported shapes and unavailable
//! statistics retain their existing plans.
use crate::rules::to_storage_predicate;
use kaveon_core::{
    BinaryOp, CatalogManager, CompareOp, DataFormat, Expr, ScalarValue, StatValue,
    StoragePredicate, StorageType, TableReference, TableStatistics,
};
use kaveon_sql::logical_plan::{JoinDistribution, JoinType, LogicalPlan};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::{collections::HashMap, sync::Arc};

const BROADCAST_BUILD_MAX_ROWS: u64 = 1_000_000;
/// A build side whose files exceed this many bytes as stored is not
/// broadcast whatever its row count; a bound the statistics add.
const BROADCAST_BUILD_MAX_BYTES: u64 = 256 * 1024 * 1024;
const BROADCAST_MIN_PROBE_TO_BUILD_RATIO: u64 = 4;

/// What planning knows about one relation.
#[derive(Clone, Debug)]
pub struct RelationStatistics {
    /// The exact row count of the source at the pinned version.
    pub rows: u64,
    pub columns: Vec<String>,
    /// The table's statistics on record — fresh or stale, for costing.
    pub table: Option<Arc<TableStatistics>>,
}

impl RelationStatistics {
    pub fn exact(rows: u64, columns: Vec<String>) -> Self {
        Self {
            rows,
            columns,
            table: None,
        }
    }

    /// The bytes as stored, when the statistics record them.
    fn bytes(&self) -> Option<u64> {
        self.table.as_ref().map(|table| table.bytes)
    }
}

pub fn optimize_join_builds(plan: LogicalPlan, catalog: &CatalogManager) -> LogicalPlan {
    let mut cache = HashMap::new();
    optimize_with_statistics(plan, &mut |table| {
        cache
            .entry(table.to_owned())
            .or_insert_with(|| {
                let resolved = catalog.resolve_table(&TableReference::parse(table)).ok()?;
                if !matches!(resolved.storage, StorageType::Local { .. }) {
                    return None;
                }
                let metadata = match resolved.table.format {
                    DataFormat::Parquet => {
                        ParquetReader::new(resolved.full_path()).metadata().ok()?
                    }
                    DataFormat::Delta => DeltaTableReader::new(resolved.full_path())
                        .metadata()
                        .ok()?,
                    DataFormat::Iceberg => return None,
                };
                Some(RelationStatistics::exact(
                    metadata.row_count,
                    metadata
                        .schema
                        .fields()
                        .iter()
                        .map(|field| field.name().clone())
                        .collect(),
                ))
            })
            .clone()
    })
}

/// A relation's estimated cardinality and the bytes behind it.
#[derive(Clone, Debug)]
struct Estimate {
    rows: u64,
    bytes: Option<u64>,
    alias: String,
    columns: Vec<String>,
}

pub fn optimize_with_statistics(
    plan: LogicalPlan,
    statistics: &mut impl FnMut(&str) -> Option<RelationStatistics>,
) -> LogicalPlan {
    match plan {
        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            distribution: _,
        } => {
            let left = Box::new(optimize_with_statistics(*left, statistics));
            let right = Box::new(optimize_with_statistics(*right, statistics));
            let left_relation = relation(&left, statistics);
            let right_relation = relation(&right, statistics);
            let swap = if join_type == JoinType::Inner {
                left_relation.clone().zip(right_relation.clone()).and_then(
                    |(left_estimate, right_estimate)| {
                        if left_estimate.rows >= right_estimate.rows
                            || left_estimate.alias == right_estimate.alias
                        {
                            return None;
                        }
                        let reversed = reverse_keys(
                            condition.as_ref()?,
                            &left_estimate.alias,
                            &right_estimate.alias,
                        )?;
                        let columns = left_estimate
                            .columns
                            .into_iter()
                            .map(|column| format!("{}.{column}", left_estimate.alias))
                            .chain(
                                right_estimate
                                    .columns
                                    .into_iter()
                                    .map(|column| format!("{}.{column}", right_estimate.alias)),
                            )
                            .map(|name| Expr::Alias {
                                expr: Box::new(Expr::Column(name.clone())),
                                name,
                            })
                            .collect();
                        Some((reversed, columns))
                    },
                )
            } else {
                None
            };
            if let Some((condition, columns)) = swap {
                let distribution =
                    join_distribution(join_type, right_relation.as_ref(), left_relation.as_ref());
                LogicalPlan::Project {
                    input: Box::new(LogicalPlan::Join {
                        left: right,
                        right: left,
                        join_type,
                        condition: Some(condition),
                        distribution,
                    }),
                    columns,
                }
            } else {
                let distribution =
                    join_distribution(join_type, left_relation.as_ref(), right_relation.as_ref());
                LogicalPlan::Join {
                    left,
                    right,
                    join_type,
                    condition,
                    distribution,
                }
            }
        }
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            columns,
        },
        LogicalPlan::Filter { input, predicate } => LogicalPlan::Filter {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            predicate,
        },
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => LogicalPlan::Aggregate {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            group_by,
            aggregates,
        },
        LogicalPlan::Sort { input, order_by } => LogicalPlan::Sort {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            order_by,
        },
        LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            count,
        },
        LogicalPlan::Offset { input, count } => LogicalPlan::Offset {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            count,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(optimize_with_statistics(*input, statistics)),
        },
        LogicalPlan::Window {
            input,
            window_exprs,
        } => LogicalPlan::Window {
            input: Box::new(optimize_with_statistics(*input, statistics)),
            window_exprs,
        },
        LogicalPlan::Union { inputs, all } => LogicalPlan::Union {
            inputs: inputs
                .into_iter()
                .map(|input| optimize_with_statistics(input, statistics))
                .collect(),
            all,
        },
        LogicalPlan::Intersect { left, right } => LogicalPlan::Intersect {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
        },
        LogicalPlan::Except { left, right } => LogicalPlan::Except {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
        },
        LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
            residual,
        } => LogicalPlan::SemiJoin {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            left_key,
            right_key,
            residual,
        },
        LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
            residual,
        } => LogicalPlan::AntiJoin {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            left_key,
            right_key,
            residual,
        },
        scan @ LogicalPlan::Scan { .. } => scan,
    }
}

fn join_distribution(
    join_type: JoinType,
    probe: Option<&Estimate>,
    build: Option<&Estimate>,
) -> JoinDistribution {
    let Some((probe, build)) = probe.zip(build) else {
        return JoinDistribution::Partitioned;
    };
    if join_type == JoinType::Inner
        && build.rows <= BROADCAST_BUILD_MAX_ROWS
        && build
            .bytes
            .is_none_or(|bytes| bytes <= BROADCAST_BUILD_MAX_BYTES)
        && probe.rows
            >= build
                .rows
                .saturating_mul(BROADCAST_MIN_PROBE_TO_BUILD_RATIO)
    {
        JoinDistribution::BroadcastRight
    } else {
        JoinDistribution::Partitioned
    }
}

/// The estimate for a direct scan, or a filter straight over one whose
/// cardinality the table's statistics can estimate; `None` for anything
/// else (reordering a derived relation needs output-schema contracts this
/// pass does not have).
fn relation(
    plan: &LogicalPlan,
    stats: &mut impl FnMut(&str) -> Option<RelationStatistics>,
) -> Option<Estimate> {
    let (scan, predicate) = match plan {
        LogicalPlan::Filter { input, predicate } => (input.as_ref(), Some(predicate)),
        other => (other, None),
    };
    let LogicalPlan::Scan {
        table,
        alias,
        columns,
    } = scan
    else {
        return None;
    };
    let stats = stats(table)?;
    let alias = alias
        .clone()
        .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned());
    let selectivity = match predicate {
        Some(predicate) => stats
            .table
            .as_ref()
            .map_or(1.0, |table| filter_selectivity(predicate, table)),
        None => 1.0,
    };
    let rows = (stats.rows as f64 * selectivity).round() as u64;
    let bytes = stats
        .bytes()
        .map(|bytes| (bytes as f64 * selectivity).round() as u64);
    Some(Estimate {
        rows,
        bytes,
        alias,
        columns: columns.clone().unwrap_or(stats.columns),
    })
}

/// The fraction of a table's rows a filter keeps, from the table's
/// statistics: an upper bound (1.0) for anything the statistics cannot
/// judge, so an estimate never understates a side.
pub fn filter_selectivity(predicate: &Expr, table: &TableStatistics) -> f64 {
    let Some(predicate) = to_storage_predicate(predicate) else {
        return 1.0;
    };
    predicate_selectivity(&predicate, table).clamp(0.0, 1.0)
}

/// The same over a storage predicate (a pushed-down scan filter).
pub fn predicate_selectivity(predicate: &StoragePredicate, table: &TableStatistics) -> f64 {
    let column = |name: &str| {
        table
            .column(name)
            .or_else(|| table.column(name.rsplit('.').next().unwrap_or(name)))
    };
    let selectivity = match predicate {
        StoragePredicate::Compare {
            column: name,
            op,
            value,
        } => column(name).map_or(1.0, |column| {
            compare_selectivity(column, *op, value, table.rows)
        }),
        StoragePredicate::IsNull { column: name } => column(name).map_or(1.0, |column| {
            null_fraction(column, table.rows).unwrap_or(1.0)
        }),
        StoragePredicate::IsNotNull { column: name } => column(name).map_or(1.0, |column| {
            1.0 - null_fraction(column, table.rows).unwrap_or(0.0)
        }),
        StoragePredicate::In {
            column: name,
            values,
        } => column(name).map_or(1.0, |column| {
            values
                .iter()
                .map(|value| compare_selectivity(column, CompareOp::Eq, value, table.rows))
                .sum::<f64>()
        }),
        StoragePredicate::And(children) => conjunction_selectivity(children, table),
        StoragePredicate::Or(children) => children
            .iter()
            .map(|child| predicate_selectivity(child, table))
            .fold(0.0, |a, b| a + b - a * b),
        StoragePredicate::Not(inner) => match inner.as_ref() {
            // A pattern's selectivity is unknown either way.
            StoragePredicate::Like { .. } => 1.0,
            inner => 1.0 - predicate_selectivity(inner, table),
        },
        StoragePredicate::Like { .. } => 1.0,
    };
    selectivity.clamp(0.0, 1.0)
}

/// One end of a range: the value and whether it is included.
type Bound = (f64, bool);

/// A conjunction's selectivity: range terms on one numeric column
/// (`x >= a AND x < b`) are one interval, judged from the quantiles or
/// interpolated between the bounds; every other term is independent.
fn conjunction_selectivity(children: &[StoragePredicate], table: &TableStatistics) -> f64 {
    let column = |name: &str| {
        table
            .column(name)
            .or_else(|| table.column(name.rsplit('.').next().unwrap_or(name)))
    };
    // Per column: the tightest lower and upper bound among the range terms.
    let mut ranges: Vec<(&str, Option<Bound>, Option<Bound>)> = Vec::new();
    let mut rest = 1.0;
    for child in children {
        let range = match child {
            StoragePredicate::Compare {
                column: name,
                op: op @ (CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge),
                value,
            } => column(name).and_then(|stats| {
                let point = scalar_number(&value.coerced_for(&stats.data_type))?;
                Some((name.as_str(), *op, point))
            }),
            _ => None,
        };
        let Some((name, op, point)) = range else {
            rest *= predicate_selectivity(child, table);
            continue;
        };
        let entry = match ranges.iter_mut().find(|(column, _, _)| *column == name) {
            Some(entry) => entry,
            None => {
                ranges.push((name, None, None));
                ranges.last_mut().expect("just pushed")
            }
        };
        let tighter = |current: Option<Bound>, next: Bound, lower: bool| match current {
            None => Some(next),
            Some(current) => {
                let keep_next = if lower {
                    next.0 > current.0 || (next.0 == current.0 && !next.1)
                } else {
                    next.0 < current.0 || (next.0 == current.0 && !next.1)
                };
                Some(if keep_next { next } else { current })
            }
        };
        match op {
            CompareOp::Gt => entry.1 = tighter(entry.1, (point, false), true),
            CompareOp::Ge => entry.1 = tighter(entry.1, (point, true), true),
            CompareOp::Lt => entry.2 = tighter(entry.2, (point, false), false),
            _ => entry.2 = tighter(entry.2, (point, true), false),
        }
    }
    for (name, low, high) in ranges {
        let stats = column(name).expect("a ranged column has statistics");
        rest *= range_selectivity(stats, low, high, table.rows);
    }
    rest
}

/// The fraction of a column's rows inside `[low, high]` (each end open or
/// closed, or unbounded): from the quantiles when the column was read,
/// else interpolated between the column's bounds; the non-null share when
/// the statistics cannot judge it.
fn range_selectivity(
    column: &kaveon_core::ColumnStatistics,
    low: Option<Bound>,
    high: Option<Bound>,
    rows: u64,
) -> f64 {
    let non_null = 1.0 - null_fraction(column, rows).unwrap_or(0.0);
    if let (Some(low), Some(high)) = (low, high)
        && (low.0 > high.0 || (low.0 == high.0 && !(low.1 && high.1)))
    {
        return 0.0;
    }
    if let Some(quantiles) = &column.quantiles
        && !quantiles.is_empty()
    {
        return quantiles.fraction_between(low, high) * non_null;
    }
    let (Some(min), Some(max)) = (
        column.min.as_ref().and_then(StatValue::to_f64),
        column.max.as_ref().and_then(StatValue::to_f64),
    ) else {
        return non_null;
    };
    if let Some(low) = low
        && low.0 > max
    {
        return 0.0;
    }
    if let Some(high) = high
        && high.0 < min
    {
        return 0.0;
    }
    if max <= min {
        return non_null;
    }
    let position = |point: f64| ((point - min) / (max - min)).clamp(0.0, 1.0);
    let from = low.map_or(0.0, |(point, _)| position(point));
    let to = high.map_or(1.0, |(point, _)| position(point));
    (to - from).max(0.0) * non_null
}

fn null_fraction(column: &kaveon_core::ColumnStatistics, rows: u64) -> Option<f64> {
    let nulls = column.null_count?;
    (rows > 0).then(|| nulls as f64 / rows as f64)
}

/// The selectivity of `column op value` from the column's bounds, distinct
/// count and quantiles; 1.0 when they cannot judge it.
fn compare_selectivity(
    column: &kaveon_core::ColumnStatistics,
    op: CompareOp,
    value: &ScalarValue,
    rows: u64,
) -> f64 {
    let value = value.coerced_for(&column.data_type);
    let non_null = 1.0 - null_fraction(column, rows).unwrap_or(0.0);
    let min = column.min.as_ref().and_then(StatValue::to_scalar);
    let max = column.max.as_ref().and_then(StatValue::to_scalar);
    if !kaveon_core::statistics::bounds_may_match(min.clone(), max.clone(), op, &value) {
        return 0.0;
    }
    match op {
        CompareOp::Eq => match column.distinct_count() {
            Some(distinct) if distinct > 0 => (non_null / distinct as f64).min(non_null),
            _ => non_null,
        },
        CompareOp::Ne => match column.distinct_count() {
            Some(distinct) if distinct > 0 => non_null * (1.0 - 1.0 / distinct as f64),
            _ => non_null,
        },
        CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge => {
            let Some(point) = scalar_number(&value) else {
                return non_null;
            };
            match op {
                CompareOp::Lt => range_selectivity(column, None, Some((point, false)), rows),
                CompareOp::Le => range_selectivity(column, None, Some((point, true)), rows),
                CompareOp::Gt => range_selectivity(column, Some((point, false)), None, rows),
                _ => range_selectivity(column, Some((point, true)), None, rows),
            }
        }
    }
}

fn scalar_number(value: &ScalarValue) -> Option<f64> {
    match value {
        ScalarValue::Int64(value) => Some(*value as f64),
        ScalarValue::Float64(value) => Some(*value),
        ScalarValue::Decimal128 { value, scale, .. } => {
            Some(*value as f64 / 10f64.powi(i32::from(*scale)))
        }
        _ => None,
    }
}

fn reverse_keys(condition: &Expr, left_alias: &str, right_alias: &str) -> Option<Expr> {
    match condition {
        Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } => {
            let (Expr::Column(a), Expr::Column(b)) = (left.as_ref(), right.as_ref()) else {
                return None;
            };
            if !a.starts_with(&format!("{left_alias}."))
                || !b.starts_with(&format!("{right_alias}."))
            {
                return None;
            }
            Some(Expr::BinaryOp {
                left: right.clone(),
                op: BinaryOp::Eq,
                right: left.clone(),
            })
        }
        Expr::And(left, right) => Some(Expr::And(
            Box::new(reverse_keys(left, left_alias, right_alias)?),
            Box::new(reverse_keys(right, left_alias, right_alias)?),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use kaveon_core::{
        ColumnStatistics, HllSketch, KllSketch, SourceVersion, SourceVersionKind, StatisticsDepth,
        TableId,
    };
    use kaveon_sql::logical_plan::sql_to_logical_plan;
    fn stats(table: &str) -> Option<RelationStatistics> {
        Some(RelationStatistics::exact(
            if table == "small" { 10 } else { 10_000 },
            vec!["id".into(), "value".into()],
        ))
    }
    #[test]
    fn puts_smaller_input_on_build_side_and_preserves_star_column_order() {
        let plan = sql_to_logical_plan("SELECT * FROM small s JOIN big b ON s.id=b.id").unwrap();
        let plan = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("restoring projection missing")
        };
        assert_eq!(
            columns
                .iter()
                .map(|expr| match expr {
                    Expr::Alias { name, .. } => name.as_str(),
                    _ => panic!("qualified alias missing"),
                })
                .collect::<Vec<_>>(),
            vec!["s.id", "s.value", "b.id", "b.value"]
        );
        let LogicalPlan::Join {
            right,
            condition,
            distribution,
            ..
        } = *input
        else {
            panic!("join missing")
        };
        assert!(matches!(*right,LogicalPlan::Scan {table,..} if table=="small"));
        assert!(format!("{condition:?}").contains("left: Column(\"b.id\")"));
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    fn estimate(rows: u64, bytes: Option<u64>) -> Estimate {
        Estimate {
            rows,
            bytes,
            alias: "t".into(),
            columns: vec![],
        }
    }

    #[test]
    fn broadcasts_only_a_proven_small_materially_smaller_inner_build() {
        assert_eq!(
            join_distribution(
                JoinType::Inner,
                Some(&estimate(4_000_000, None)),
                Some(&estimate(1_000_000, None))
            ),
            JoinDistribution::BroadcastRight
        );
        for (join_type, probe, build) in [
            (JoinType::Left, 4_000_000, Some(1_000_000)),
            (JoinType::Inner, 3_999_999, Some(1_000_000)),
            (JoinType::Inner, 8_000_000, Some(1_000_001)),
            (JoinType::Inner, 10, None),
        ] {
            assert_eq!(
                join_distribution(
                    join_type,
                    Some(&estimate(probe, None)),
                    build.map(|rows| estimate(rows, None)).as_ref()
                ),
                JoinDistribution::Partitioned
            );
        }
        // Statistics add a byte bound the row count alone cannot see.
        assert_eq!(
            join_distribution(
                JoinType::Inner,
                Some(&estimate(4_000_000, None)),
                Some(&estimate(1_000, Some(BROADCAST_BUILD_MAX_BYTES + 1)))
            ),
            JoinDistribution::Partitioned
        );
    }
    #[test]
    fn leaves_outer_unknown_and_unqualified_joins_unchanged() {
        for sql in [
            "SELECT * FROM small s LEFT JOIN big b ON s.id=b.id",
            "SELECT * FROM small s FULL JOIN big b ON s.id=b.id",
            "SELECT * FROM small s JOIN big b ON id=id",
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let before = format!("{plan:?}");
            assert_eq!(
                format!("{:?}", optimize_with_statistics(plan, &mut stats)),
                before
            );
        }
        let plan = sql_to_logical_plan("SELECT * FROM small s JOIN big b ON s.id=b.id").unwrap();
        let before = format!("{plan:?}");
        assert_eq!(
            format!("{:?}", optimize_with_statistics(plan, &mut |_| None)),
            before
        );
    }

    fn table_statistics(rows: u64) -> TableStatistics {
        let mut distinct = HllSketch::default_precision();
        for value in 0..1000u64 {
            distinct.insert_text(&value.to_string());
        }
        let mut quantiles = KllSketch::default_k();
        for value in 0..rows {
            quantiles.update((value % 1000) as f64);
        }
        TableStatistics {
            version: kaveon_core::statistics::TABLE_STATISTICS_VERSION,
            table_id: TableId::new("table:events").unwrap(),
            source_version: SourceVersion {
                identity_sha256: "abc".into(),
                kind: SourceVersionKind::File,
            },
            computed_at_ms: 0,
            depth: StatisticsDepth::Full,
            format: kaveon_core::DataFormat::Parquet,
            location: "/lake/t".into(),
            rows,
            bytes: rows * 100,
            files: 1,
            row_groups: None,
            uncompressed_bytes: None,
            last_modified_ms: None,
            partition_columns: Vec::new(),
            columns: vec![
                ColumnStatistics {
                    name: "id".into(),
                    data_type: DataType::Int64,
                    null_count: Some(0),
                    min: Some(StatValue::Int(0)),
                    max: Some(StatValue::Int(999)),
                    bounds_exact: true,
                    distinct: Some(distinct),
                    distinct_exact: None,
                    quantiles: Some(quantiles),
                    bytes: None,
                },
                ColumnStatistics {
                    name: "region".into(),
                    data_type: DataType::Utf8,
                    null_count: Some(rows / 10),
                    min: Some(StatValue::Text("east".into())),
                    max: Some(StatValue::Text("west".into())),
                    bounds_exact: true,
                    distinct: None,
                    distinct_exact: Some(4),
                    quantiles: None,
                    bytes: None,
                },
            ],
            per_file: Vec::new(),
            per_file_complete: false,
        }
    }

    /// The predicate of `SELECT * FROM events WHERE <sql_predicate>`.
    fn predicate_of(sql_predicate: &str) -> Expr {
        let plan =
            sql_to_logical_plan(&format!("SELECT * FROM events WHERE {sql_predicate}")).unwrap();
        let filter = match plan {
            LogicalPlan::Project { input, .. } => *input,
            other => other,
        };
        let LogicalPlan::Filter { predicate, .. } = filter else {
            panic!("filter");
        };
        predicate
    }

    fn selectivity(sql_predicate: &str) -> f64 {
        filter_selectivity(&predicate_of(sql_predicate), &table_statistics(1_000_000))
    }

    #[test]
    fn selectivity_follows_distinct_counts_bounds_nulls_and_quantiles() {
        let close = |actual: f64, expected: f64| {
            assert!((actual - expected).abs() < 0.02, "{actual} vs {expected}");
        };
        close(selectivity("id = 5"), 0.001);
        close(selectivity("id <> 5"), 0.999);
        close(selectivity("id = 5000"), 0.0);
        close(selectivity("id < 0"), 0.0);
        close(selectivity("id < 250"), 0.25);
        close(selectivity("id >= 750"), 0.25);
        close(selectivity("id BETWEEN 100 AND 199"), 0.1);
        close(
            selectivity("id >= 100 AND id < 200 AND region = 'east'"),
            0.1 * 0.9 / 4.0,
        );
        close(selectivity("id > 500 AND id < 400"), 0.0);
        close(selectivity("region = 'east'"), 0.9 / 4.0);
        close(selectivity("region IS NULL"), 0.1);
        close(selectivity("region IS NOT NULL"), 0.9);
        close(selectivity("region IN ('east', 'west')"), 0.45);
        close(selectivity("id < 250 OR id >= 750"), 0.4375);
        close(selectivity("NOT (id < 250)"), 0.75);
        close(selectivity("region LIKE 'e%'"), 1.0);
        close(selectivity("region = 'zulu'"), 0.0);
        close(selectivity("other = 1"), 1.0);
        // Without quantiles, a range interpolates between the bounds.
        let mut table = table_statistics(1_000_000);
        table.columns[0].quantiles = None;
        close(
            filter_selectivity(&predicate_of("id < 250"), &table),
            250.0 / 999.0,
        );
        close(
            filter_selectivity(&predicate_of("id BETWEEN 100 AND 199"), &table),
            99.0 / 999.0,
        );
        close(filter_selectivity(&predicate_of("id > 2000"), &table), 0.0);
    }

    #[test]
    fn a_filtered_scan_with_statistics_becomes_the_build_side_by_estimated_rows() {
        // Two tables of the same size; the filter on events keeps 0.1 % and
        // the statistics know it, so events broadcasts under other.
        let mut lookup = |table: &str| {
            Some(RelationStatistics {
                rows: 1_000_000,
                columns: vec!["id".into(), "region".into()],
                table: (table == "events").then(|| Arc::new(table_statistics(1_000_000))),
            })
        };
        let plan = sql_to_logical_plan(
            "SELECT * FROM events e JOIN other o ON e.id = o.id WHERE e.id = 5",
        )
        .unwrap();
        let plan = crate::rules::push_filter_down(plan);
        let plan = optimize_with_statistics(plan, &mut lookup);
        fn find_join(plan: &LogicalPlan) -> Option<(&LogicalPlan, &LogicalPlan, JoinDistribution)> {
            match plan {
                LogicalPlan::Join {
                    left,
                    right,
                    distribution,
                    ..
                } => Some((left, right, *distribution)),
                LogicalPlan::Project { input, .. } | LogicalPlan::Filter { input, .. } => {
                    find_join(input)
                }
                _ => None,
            }
        }
        let (left, right, distribution) = find_join(&plan).expect("a join");
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
        assert!(matches!(left, LogicalPlan::Scan { table, .. } if table == "other"));
        assert!(matches!(right, LogicalPlan::Filter { .. }));
        // Without the table's statistics the filtered side is an upper
        // bound of equal size: no swap, no broadcast — the plan of today.
        let plan = sql_to_logical_plan(
            "SELECT * FROM events e JOIN other o ON e.id = o.id WHERE e.id = 5",
        )
        .unwrap();
        let plan = crate::rules::push_filter_down(plan);
        let plan = optimize_with_statistics(plan, &mut |_| {
            Some(RelationStatistics::exact(
                1_000_000,
                vec!["id".into(), "region".into()],
            ))
        });
        let (left, _, distribution) = find_join(&plan).expect("a join");
        assert_eq!(distribution, JoinDistribution::Partitioned);
        assert!(matches!(left, LogicalPlan::Filter { .. }));
    }
}
