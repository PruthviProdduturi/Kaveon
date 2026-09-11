//! Conservative build-side selection using exact local scan row counts.
//! Unsupported shapes and unavailable statistics retain their existing plans.
use kaveon_core::{BinaryOp, CatalogManager, DataFormat, Expr, StorageType, TableReference};
use kaveon_sql::logical_plan::{JoinDistribution, JoinType, LogicalPlan};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::collections::HashMap;

const BROADCAST_BUILD_MAX_ROWS: u64 = 1_000_000;
const BROADCAST_MIN_PROBE_TO_BUILD_RATIO: u64 = 4;

#[derive(Clone, Debug)]
pub struct RelationStatistics {
    pub rows: u64,
    pub columns: Vec<String>,
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
                Some(RelationStatistics {
                    rows: metadata.row_count,
                    columns: metadata
                        .schema
                        .fields()
                        .iter()
                        .map(|field| field.name().clone())
                        .collect(),
                })
            })
            .clone()
    })
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
                left_relation
                    .clone()
                    .zip(right_relation.clone())
                    .and_then(
                        |(
                            (left_rows, left_alias, left_columns),
                            (right_rows, right_alias, right_columns),
                        )| {
                            if left_rows >= right_rows || left_alias == right_alias {
                                return None;
                            }
                            let reversed =
                                reverse_keys(condition.as_ref()?, &left_alias, &right_alias)?;
                            let columns = left_columns
                                .into_iter()
                                .map(|column| format!("{left_alias}.{column}"))
                                .chain(
                                    right_columns
                                        .into_iter()
                                        .map(|column| format!("{right_alias}.{column}")),
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
                let distribution = join_distribution(
                    join_type,
                    right_relation.as_ref().map(|value| value.0),
                    left_relation.as_ref().map(|value| value.0),
                );
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
                let distribution = join_distribution(
                    join_type,
                    left_relation.as_ref().map(|value| value.0),
                    right_relation.as_ref().map(|value| value.0),
                );
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
        } => LogicalPlan::SemiJoin {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            left_key,
            right_key,
        },
        LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
        } => LogicalPlan::AntiJoin {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            left_key,
            right_key,
        },
        scan @ LogicalPlan::Scan { .. } => scan,
    }
}

fn join_distribution(
    join_type: JoinType,
    probe_rows: Option<u64>,
    build_rows: Option<u64>,
) -> JoinDistribution {
    let Some((probe_rows, build_rows)) = probe_rows.zip(build_rows) else {
        return JoinDistribution::Partitioned;
    };
    if join_type == JoinType::Inner
        && build_rows <= BROADCAST_BUILD_MAX_ROWS
        && probe_rows >= build_rows.saturating_mul(BROADCAST_MIN_PROBE_TO_BUILD_RATIO)
    {
        JoinDistribution::BroadcastRight
    } else {
        JoinDistribution::Partitioned
    }
}

fn relation(
    plan: &LogicalPlan,
    stats: &mut impl FnMut(&str) -> Option<RelationStatistics>,
) -> Option<(u64, String, Vec<String>)> {
    // Restrict to direct scans. Reordering a filtered/derived relation requires
    // complete output-schema and cardinality contracts, not guessed qualifiers.
    let LogicalPlan::Scan {
        table,
        alias,
        columns,
    } = plan
    else {
        return None;
    };
    let stats = stats(table)?;
    Some((
        stats.rows,
        alias
            .clone()
            .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned()),
        columns.clone().unwrap_or(stats.columns),
    ))
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
    use kaveon_sql::logical_plan::sql_to_logical_plan;
    fn stats(table: &str) -> Option<RelationStatistics> {
        Some(RelationStatistics {
            rows: if table == "small" { 10 } else { 10_000 },
            columns: vec!["id".into(), "value".into()],
        })
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

    #[test]
    fn broadcasts_only_a_proven_small_materially_smaller_inner_build() {
        assert_eq!(
            join_distribution(JoinType::Inner, Some(4_000_000), Some(1_000_000)),
            JoinDistribution::BroadcastRight
        );
        for (join_type, probe, build) in [
            (JoinType::Left, Some(4_000_000), Some(1_000_000)),
            (JoinType::Inner, Some(3_999_999), Some(1_000_000)),
            (JoinType::Inner, Some(8_000_000), Some(1_000_001)),
            (JoinType::Inner, None, Some(10)),
        ] {
            assert_eq!(
                join_distribution(join_type, probe, build),
                JoinDistribution::Partitioned
            );
        }
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
}
