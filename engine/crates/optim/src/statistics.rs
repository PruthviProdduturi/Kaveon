//! Build-side selection and join placement from what the catalog knows:
//! exact row counts, and — when a table's statistics are on record —
//! bounds, null counts, distinct counts and quantiles that turn a filtered
//! scan into an estimated cardinality. Unsupported shapes and unavailable
//! statistics retain their existing plans.
use crate::rules::to_storage_predicate;
use kaveon_core::{
    BinaryOp, CatalogManager, CompareOp, DataFormat, Expr, ResolvedTable, ScalarValue, StatValue,
    StoragePredicate, StorageType, TableReference, TableStatistics,
};
use kaveon_sql::logical_plan::{JoinDistribution, JoinType, LogicalPlan};
use kaveon_storage::{DeltaTableReader, IcebergReader, ParquetReader, SourceStatistics};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

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
    /// Exact bytes from the pinned source metadata, when available.
    pub bytes_hint: Option<u64>,
    /// Column statistics for costing; `from_source` accepts only the
    /// document matching the pinned source identity.
    pub table: Option<Arc<TableStatistics>>,
}

impl RelationStatistics {
    pub fn exact(rows: u64, columns: Vec<String>) -> Self {
        Self {
            rows,
            columns,
            bytes_hint: None,
            table: None,
        }
    }

    pub fn exact_with_bytes(rows: u64, columns: Vec<String>, bytes: u64) -> Self {
        Self {
            rows,
            columns,
            bytes_hint: Some(bytes),
            table: None,
        }
    }

    /// Use source facts from one identity. Stale column statistics cannot
    /// shrink a current build's row or byte estimate for broadcast admission.
    pub fn from_source(source: &SourceStatistics, table: Option<Arc<TableStatistics>>) -> Self {
        Self {
            rows: source.row_count,
            columns: source.columns.clone(),
            bytes_hint: source.bytes,
            table: table.filter(|table| table.is_current_for(&source.identity_sha256)),
        }
    }

    /// The bytes as stored, from exact source metadata. A statistics
    /// document may itself have incomplete file sizes, so it is not a
    /// fallback for an absent source bound.
    fn bytes(&self) -> Option<u64> {
        self.bytes_hint
    }
}

fn relation_statistics(resolved: &ResolvedTable) -> Option<RelationStatistics> {
    if !matches!(resolved.storage, StorageType::Local { .. }) {
        return None;
    }
    match resolved.table.format {
        DataFormat::Parquet => {
            let metadata = ParquetReader::new(resolved.full_path()).metadata().ok()?;
            Some(RelationStatistics::exact_with_bytes(
                metadata.row_count,
                metadata
                    .schema
                    .fields()
                    .iter()
                    .map(|field| field.name().clone())
                    .collect(),
                metadata.profile.file_bytes,
            ))
        }
        DataFormat::Delta => {
            let metadata = DeltaTableReader::new(resolved.full_path())
                .metadata()
                .ok()?;
            Some(RelationStatistics::exact_with_bytes(
                metadata.row_count,
                metadata
                    .schema
                    .fields()
                    .iter()
                    .map(|field| field.name().clone())
                    .collect(),
                metadata.profile.file_bytes,
            ))
        }
        DataFormat::Iceberg => {
            let snapshot = IcebergReader::new(resolved.full_path()).snapshot().ok()?;
            Some(RelationStatistics::exact_with_bytes(
                snapshot.row_count,
                snapshot
                    .schema
                    .fields()
                    .iter()
                    .map(|field| field.name().clone())
                    .collect(),
                snapshot.total_bytes,
            ))
        }
    }
}

pub fn optimize_join_builds(plan: LogicalPlan, catalog: &CatalogManager) -> LogicalPlan {
    let mut cache = HashMap::new();
    optimize_with_statistics(plan, &mut |table| {
        cache
            .entry(table.to_owned())
            .or_insert_with(|| {
                let resolved = catalog.resolve_table(&TableReference::parse(table)).ok()?;
                relation_statistics(&resolved)
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
            using_columns,
            ..
        } => {
            // The parser builds a left-deep tree for a join chain. Before
            // optimizing each binary join independently, enumerate the two
            // connected pairings in a three-relation inner equi-join chain.
            // This is deliberately bounded: the rewrite needs exact
            // relation facts and leaves outer, USING, residual, and larger
            // join graphs to the existing conservative rules.
            if let Some(reordered) = reorder_left_deep_inner(
                (*left).clone(),
                (*right).clone(),
                join_type,
                condition.as_ref(),
                using_columns.as_ref(),
                statistics,
            ) {
                return reordered;
            }
            if let Some(reordered) = reorder_three_way_inner(
                (*left).clone(),
                (*right).clone(),
                join_type,
                condition.as_ref(),
                using_columns.as_ref(),
                statistics,
            ) {
                return reordered;
            }
            let left = Box::new(optimize_with_statistics(*left, statistics));
            let right = Box::new(optimize_with_statistics(*right, statistics));
            let left_relation = relation(&left, statistics);
            let right_relation = relation(&right, statistics);
            let swap = if join_type == JoinType::Inner {
                left_relation.clone().zip(right_relation.clone()).and_then(
                    |(left_estimate, right_estimate)| {
                        if !left_build_is_cheaper(&left_estimate, &right_estimate)
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
                        using_columns: None,
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
                    using_columns,
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
        LogicalPlan::Intersect { left, right, all } => LogicalPlan::Intersect {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            all,
        },
        LogicalPlan::Except { left, right, all } => LogicalPlan::Except {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            all,
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
            null_aware,
        } => LogicalPlan::AntiJoin {
            left: Box::new(optimize_with_statistics(*left, statistics)),
            right: Box::new(optimize_with_statistics(*right, statistics)),
            left_key,
            right_key,
            residual,
            null_aware,
        },
        scan @ LogicalPlan::Scan { .. } => scan,
        unnest @ LogicalPlan::Unnest { .. } => unnest,
    }
}

/// Reorder a bounded left-deep inner equi-join chain when every leaf has exact
/// source statistics. The parser emits a left-deep tree, so the search only
/// considers connected left-deep build orders. Keeping the cap small prevents
/// factorial planning work from becoming a query-time denial of service while
/// covering the common star/snowflake join shapes. Unsupported outer,
/// `USING`, and residual joins remain on the conservative path.
fn reorder_left_deep_inner(
    left: LogicalPlan,
    right: LogicalPlan,
    join_type: JoinType,
    condition: Option<&Expr>,
    using_columns: Option<&Vec<String>>,
    statistics: &mut impl FnMut(&str) -> Option<RelationStatistics>,
) -> Option<LogicalPlan> {
    if join_type != JoinType::Inner || condition.is_none() || using_columns.is_some() {
        return None;
    }
    let mut leaves = Vec::with_capacity(6);
    let mut conditions = Vec::with_capacity(5);
    let mut current = left;
    loop {
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            using_columns,
            ..
        } = current
        else {
            leaves.push(current);
            break;
        };
        if join_type != JoinType::Inner || condition.is_none() || using_columns.is_some() {
            return None;
        }
        conditions.push(condition?);
        current = *left;
        leaves.push(*right);
    }
    // The loop collected the inner leaves in reverse nesting order.
    // Reconstruct the written left-deep order, then append the top relation.
    leaves.reverse();
    leaves.push(right);
    conditions.reverse();
    conditions.push(condition?.clone());
    if !(4..=6).contains(&leaves.len()) || conditions.len() != leaves.len() - 1 {
        return None;
    }

    let estimates = leaves
        .iter()
        .map(|leaf| relation(leaf, statistics))
        .collect::<Option<Vec<_>>>()?;
    let aliases = estimates
        .iter()
        .map(|estimate| estimate.alias.clone())
        .collect::<Vec<_>>();
    if aliases.iter().collect::<HashSet<_>>().len() != leaves.len() {
        return None;
    }
    let edges = conditions
        .iter()
        .map(|condition| {
            let pair = condition_alias_pair(condition)?;
            let left = aliases.iter().position(|alias| alias == &pair.0)?;
            let right = aliases.iter().position(|alias| alias == &pair.1)?;
            Some((condition, left, right))
        })
        .collect::<Option<Vec<_>>>()?;
    if edges.iter().any(|(_, left, right)| left == right) {
        return None;
    }

    // The recursive search carries immutable plan inputs plus the evolving
    // state explicitly; keeping those arguments visible makes the bounded
    // enumeration easier to audit than hiding them in mutable global state.
    #[allow(clippy::too_many_arguments)]
    fn search(
        leaves: &[LogicalPlan],
        estimates: &[Estimate],
        aliases: &[String],
        edges: &[(&Expr, usize, usize)],
        selected: Vec<usize>,
        plan: LogicalPlan,
        cost: JoinCost,
        best: &mut Option<(JoinCost, LogicalPlan)>,
    ) {
        if selected.len() == leaves.len() {
            if best.as_ref().is_none_or(|(previous, _)| cost < *previous) {
                *best = Some((cost, plan));
            }
            return;
        }
        let selected_set = selected.iter().copied().collect::<HashSet<_>>();
        for (condition, left, right) in edges {
            let new_index = if selected_set.contains(left) && !selected_set.contains(right) {
                *right
            } else if selected_set.contains(right) && !selected_set.contains(left) {
                *left
            } else {
                continue;
            };
            if selected.contains(&new_index) {
                continue;
            }
            let left_aliases = selected
                .iter()
                .map(|index| aliases[*index].as_str())
                .collect::<Vec<_>>();
            let oriented =
                match orient_condition(condition, &left_aliases, &[aliases[new_index].as_str()]) {
                    Some(oriented) => oriented,
                    None => continue,
                };
            let next_cost = cost.then_attach(&estimates[new_index]);
            let next_plan = LogicalPlan::Join {
                left: Box::new(plan.clone()),
                right: Box::new(leaves[new_index].clone()),
                join_type: JoinType::Inner,
                condition: Some(oriented),
                using_columns: None,
                distribution: join_distribution(
                    JoinType::Inner,
                    Some(&Estimate {
                        rows: cost.rows,
                        bytes: Some(cost.bytes),
                        alias: "__join".into(),
                        columns: selected
                            .iter()
                            .flat_map(|index| estimates[*index].columns.iter().cloned())
                            .collect(),
                    }),
                    Some(&estimates[new_index]),
                ),
            };
            let mut next_selected = selected.clone();
            next_selected.push(new_index);
            search(
                leaves,
                estimates,
                aliases,
                edges,
                next_selected,
                next_plan,
                next_cost,
                best,
            );
        }
    }

    let mut best = None;
    for (condition, left, right) in &edges {
        for (first, second) in [(*left, *right), (*right, *left)] {
            let first_join = build_inner_join(
                leaves[first].clone(),
                leaves[second].clone(),
                condition,
                aliases[first].as_str(),
                aliases[second].as_str(),
                &estimates[first],
                &estimates[second],
            )?;
            let selected = vec![first, second];
            let cost = JoinCost::for_pair(&estimates[first], &estimates[second]);
            search(
                &leaves, &estimates, &aliases, &edges, selected, first_join, cost, &mut best,
            );
        }
    }
    let (_, plan) = best?;
    let columns = estimates
        .iter()
        .flat_map(|estimate| {
            estimate.columns.iter().map(|column| {
                let name = format!("{}.{column}", estimate.alias);
                Expr::Alias {
                    expr: Box::new(Expr::Column(name.clone())),
                    name,
                }
            })
        })
        .collect();
    Some(LogicalPlan::Project {
        input: Box::new(plan),
        columns,
    })
}

/// Prefer the relation with the smaller known serialized footprint when both
/// sides expose byte statistics. Fall back to row counts when bytes are
/// unavailable on either side; mixing a known byte value with an unknown one
/// would make the comparison appear more precise than it is.
fn left_build_is_cheaper(left: &Estimate, right: &Estimate) -> bool {
    match (left.bytes, right.bytes) {
        (Some(left_bytes), Some(right_bytes)) if left_bytes != right_bytes => {
            left_bytes < right_bytes
        }
        _ => left.rows < right.rows,
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
    // A right-side broadcast is also semantics-preserving for a left outer
    // join: the probe side remains the preserved left relation, while the
    // build side is replicated exactly as in the partitioned operator.  Do
    // not apply this to RIGHT/FULL joins because that would require swapping
    // the preserved side or a left-broadcast distribution.
    if matches!(join_type, JoinType::Inner | JoinType::Left)
        && build.rows <= BROADCAST_BUILD_MAX_ROWS
        && build
            .bytes
            .is_some_and(|bytes| bytes <= BROADCAST_BUILD_MAX_BYTES)
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
    if let LogicalPlan::Limit { input, count } = plan {
        let mut estimate = relation(input, stats)?;
        let source_rows = estimate.rows;
        estimate.rows = estimate.rows.min(*count as u64);
        estimate.bytes = estimate.bytes.map(|bytes| {
            let selectivity = if source_rows == 0 {
                0.0
            } else {
                estimate.rows as f64 / source_rows as f64
            };
            scaled_upper_bound(bytes, selectivity)
        });
        return Some(estimate);
    }
    if let LogicalPlan::Offset { input, count } = plan {
        let mut estimate = relation(input, stats)?;
        let source_rows = estimate.rows;
        estimate.rows = source_rows.saturating_sub(*count as u64);
        estimate.bytes = estimate.bytes.map(|bytes| {
            let selectivity = if source_rows == 0 {
                0.0
            } else {
                estimate.rows as f64 / source_rows as f64
            };
            scaled_upper_bound(bytes, selectivity)
        });
        return Some(estimate);
    }
    // DISTINCT can only reduce cardinality and preserves the projected row width.
    // Retain the input upper bound so a bounded distinct build can still use
    // a safe broadcast join decision.
    if let LogicalPlan::Distinct { input } = plan {
        return relation(input, stats);
    }

    // A projection that only forwards or renames columns preserves the
    // relation's cardinality and storage footprint. Keep the exact source
    // estimate visible through that wrapper so a subquery does not disable a
    // safe broadcast decision. Computed expressions are deliberately left
    // opaque: their selectivity and width are not known here.
    if let LogicalPlan::Project { input, columns } = plan {
        let mut estimate = relation(input, stats)?;
        let mut projected = Vec::with_capacity(columns.len());
        for expression in columns {
            match expression {
                Expr::Column(name) => projected.push(name.clone()),
                Expr::Alias { expr, name } if matches!(expr.as_ref(), Expr::Column(_)) => {
                    projected.push(name.clone())
                }
                _ => return None,
            }
        }
        estimate.columns = projected;
        return Some(estimate);
    }
    // Ordering is row-preserving and does not change the stored relation
    // width. Retain exact source statistics through a sorted subquery; the
    // physical sort still accounts for its own memory and spill budget.
    if let LogicalPlan::Sort { input, .. } = plan {
        return relation(input, stats);
    }

    // A filter over a bounded/derived relation cannot make that relation
    // larger.  Keep the input's upper bound when the filter is not directly
    // over a scan; the direct-scan case below still uses column statistics to
    // estimate selectivity.  This lets a query such as
    // `FROM (SELECT ... LIMIT 1000) s WHERE ...` retain its safe broadcast
    // bound instead of falling back to a partitioned join merely because the
    // predicate is outside the subquery.
    if let LogicalPlan::Filter { input, .. } = plan
        && !matches!(input.as_ref(), LogicalPlan::Scan { .. })
    {
        return relation(input, stats);
    }

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
    let rows = scaled_upper_bound(stats.rows, selectivity);
    let bytes = stats
        .bytes()
        .map(|bytes| scaled_upper_bound(bytes, selectivity));
    Some(Estimate {
        rows,
        bytes,
        alias,
        columns: columns.clone().unwrap_or(stats.columns),
    })
}

/// Scale a source statistic without understating a non-empty filtered
/// relation. Rounding a small estimate down to zero can incorrectly make a
/// build look free and trigger an unsafe broadcast decision.
fn scaled_upper_bound(value: u64, selectivity: f64) -> u64 {
    if value == 0 || selectivity <= 0.0 {
        return 0;
    }
    let scaled = value as f64 * selectivity;
    if !scaled.is_finite() || scaled >= u64::MAX as f64 {
        u64::MAX
    } else {
        scaled.ceil() as u64
    }
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

/// Reorder one left-deep chain when all three leaves have exact source
/// statistics.  The parser's join tree for `a JOIN b ... JOIN c` contains
/// only two join edges, so there are two meaningful connected starting pairs:
/// the written pair and the pair formed by the top edge.  Exhaustively
/// comparing those two choices gives the useful part of a cost model without
/// making statistics-based rewrites speculative for arbitrary join graphs.
fn reorder_three_way_inner(
    left: LogicalPlan,
    right: LogicalPlan,
    join_type: JoinType,
    condition: Option<&Expr>,
    using_columns: Option<&Vec<String>>,
    statistics: &mut impl FnMut(&str) -> Option<RelationStatistics>,
) -> Option<LogicalPlan> {
    if join_type != JoinType::Inner || condition.is_none() || using_columns.is_some() {
        return None;
    }
    let LogicalPlan::Join {
        left: first,
        right: second,
        join_type: first_type,
        condition: first_condition,
        using_columns: first_using,
        ..
    } = left
    else {
        return None;
    };
    if first_type != JoinType::Inner || first_condition.is_none() || first_using.is_some() {
        return None;
    }

    let leaves = vec![*first, *second, right];
    let mut estimates = Vec::with_capacity(3);
    for leaf in &leaves {
        estimates.push(relation(leaf, statistics)?);
    }
    let aliases = estimates
        .iter()
        .map(|estimate| estimate.alias.clone())
        .collect::<Vec<_>>();
    if aliases.iter().collect::<HashSet<_>>().len() != 3 {
        return None;
    }
    let first_edge = condition_alias_pair(first_condition.as_ref()?)?;
    let second_edge = condition_alias_pair(condition?)?;
    if !edge_is_between(&first_edge, &aliases[0], &aliases[1]) {
        return None;
    }
    if !second_edge.0.eq(&aliases[2]) && !second_edge.1.eq(&aliases[2]) {
        return None;
    }
    if !second_edge.0.eq(&aliases[0])
        && !second_edge.1.eq(&aliases[0])
        && !second_edge.0.eq(&aliases[1])
        && !second_edge.1.eq(&aliases[1])
    {
        return None;
    }

    // Candidate 0 is the written pair. Candidate 1 starts with the pair on
    // the top edge, then attaches the remaining leaf through the other edge.
    let top_other = if second_edge.0 == aliases[2] {
        aliases.iter().position(|alias| alias == &second_edge.1)?
    } else if second_edge.1 == aliases[2] {
        aliases.iter().position(|alias| alias == &second_edge.0)?
    } else {
        return None;
    };
    let remaining = (0..3).find(|index| *index != top_other && *index != 2)?;
    let candidates = [
        (0usize, 1usize, 2usize, first_condition.as_ref()?, condition),
        (
            top_other,
            2,
            remaining,
            condition?,
            first_condition.as_ref(),
        ),
    ];
    let mut best: Option<(JoinCost, usize, LogicalPlan)> = None;
    for (candidate_index, (pair_left, pair_right, remaining, pair_condition, attach_condition)) in
        candidates.into_iter().enumerate()
    {
        if pair_left == pair_right || pair_left >= 3 || pair_right >= 3 || remaining >= 3 {
            continue;
        }
        let pair_cost = JoinCost::for_pair(&estimates[pair_left], &estimates[pair_right]);
        let cost = pair_cost.then_attach(&estimates[remaining]);
        if best
            .as_ref()
            .is_some_and(|(previous, _, _)| *previous <= cost)
        {
            continue;
        }
        let first_join = build_inner_join(
            leaves[pair_left].clone(),
            leaves[pair_right].clone(),
            pair_condition,
            &estimates[pair_left].alias,
            &estimates[pair_right].alias,
            &estimates[pair_left],
            &estimates[pair_right],
        )?;
        let intermediate = JoinCost::for_pair(&estimates[pair_left], &estimates[pair_right]);
        let remaining_alias = &estimates[remaining].alias;
        let left_aliases = [
            estimates[pair_left].alias.as_str(),
            estimates[pair_right].alias.as_str(),
        ];
        let oriented = orient_condition(attach_condition?, &left_aliases, &[remaining_alias])?;
        let intermediate_estimate = Estimate {
            rows: intermediate.rows,
            bytes: Some(intermediate.bytes),
            alias: "__join".into(),
            columns: estimates[pair_left]
                .columns
                .iter()
                .chain(estimates[pair_right].columns.iter())
                .cloned()
                .collect(),
        };
        let final_distribution = join_distribution(
            JoinType::Inner,
            Some(&intermediate_estimate),
            Some(&estimates[remaining]),
        );
        let final_plan = LogicalPlan::Join {
            left: Box::new(first_join),
            right: Box::new(leaves[remaining].clone()),
            join_type: JoinType::Inner,
            condition: Some(oriented),
            using_columns: None,
            distribution: final_distribution,
        };
        best = Some((cost, candidate_index, final_plan));
    }
    let (_, _, plan) = best?;
    // A reordered inner join changes physical field order. Keep the original
    // SQL contract explicitly, just as the existing binary swap does.
    let columns = estimates
        .iter()
        .flat_map(|estimate| {
            estimate.columns.iter().map(|column| {
                let name = format!("{}.{column}", estimate.alias);
                Expr::Alias {
                    expr: Box::new(Expr::Column(name.clone())),
                    name,
                }
            })
        })
        .collect();
    Some(LogicalPlan::Project {
        input: Box::new(plan),
        columns,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct JoinCost {
    rows: u64,
    bytes: u64,
}

impl JoinCost {
    fn for_pair(left: &Estimate, right: &Estimate) -> Self {
        Self {
            // With no join-key NDV, a product is the same for every connected
            // ordering. Use the larger input as the first-stage footprint;
            // this keeps the estimate conservative without pretending to know
            // join selectivity that the catalog does not provide.
            rows: left.rows.max(right.rows),
            bytes: match (left.bytes, right.bytes) {
                (Some(left), Some(right)) => left.max(right),
                _ => u64::MAX,
            },
        }
    }

    fn then_attach(self, relation: &Estimate) -> Self {
        Self {
            rows: self.rows.saturating_add(self.rows.max(relation.rows)),
            bytes: self
                .bytes
                .saturating_add(self.bytes.max(relation.bytes.unwrap_or(u64::MAX))),
        }
    }
}

fn build_inner_join(
    left: LogicalPlan,
    right: LogicalPlan,
    condition: &Expr,
    left_alias: &str,
    right_alias: &str,
    left_estimate: &Estimate,
    right_estimate: &Estimate,
) -> Option<LogicalPlan> {
    let condition = orient_condition(condition, &[left_alias], &[right_alias])?;
    Some(LogicalPlan::Join {
        left: Box::new(left),
        right: Box::new(right),
        join_type: JoinType::Inner,
        condition: Some(condition),
        using_columns: None,
        distribution: join_distribution(JoinType::Inner, Some(left_estimate), Some(right_estimate)),
    })
}

fn condition_alias_pair(condition: &Expr) -> Option<(String, String)> {
    let mut aliases = Vec::new();
    fn visit(expr: &Expr, aliases: &mut Vec<String>) -> Option<()> {
        match expr {
            Expr::And(left, right) => {
                visit(left, aliases)?;
                visit(right, aliases)?;
            }
            Expr::BinaryOp {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                let (Expr::Column(left), Expr::Column(right)) = (left.as_ref(), right.as_ref())
                else {
                    return None;
                };
                let left = left.split_once('.')?.0.to_owned();
                let right = right.split_once('.')?.0.to_owned();
                aliases.push(left);
                aliases.push(right);
            }
            _ => return None,
        }
        Some(())
    }
    visit(condition, &mut aliases)?;
    aliases.sort();
    aliases.dedup();
    (aliases.len() == 2).then(|| (aliases[0].clone(), aliases[1].clone()))
}

fn edge_is_between(edge: &(String, String), left: &str, right: &str) -> bool {
    (edge.0 == left && edge.1 == right) || (edge.0 == right && edge.1 == left)
}

fn orient_condition(condition: &Expr, left: &[&str], right: &[&str]) -> Option<Expr> {
    let belongs = |column: &str, group: &[&str]| {
        column
            .split_once('.')
            .is_some_and(|(alias, _)| group.contains(&alias))
    };
    match condition {
        Expr::And(first, second) => Some(Expr::And(
            Box::new(orient_condition(first, left, right)?),
            Box::new(orient_condition(second, left, right)?),
        )),
        Expr::BinaryOp {
            left: first,
            op: BinaryOp::Eq,
            right: second,
        } => {
            let (Expr::Column(first), Expr::Column(second)) = (first.as_ref(), second.as_ref())
            else {
                return None;
            };
            if belongs(first, left) && belongs(second, right) {
                Some(condition.clone())
            } else if belongs(first, right) && belongs(second, left) {
                Some(Expr::BinaryOp {
                    left: Box::new(Expr::Column(second.clone())),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Column(first.clone())),
                })
            } else {
                None
            }
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
    use arrow::datatypes::Schema;
    use kaveon_core::{
        AccessPattern, ColumnStatistics, HllSketch, KllSketch, ResolvedTable, SourceVersion,
        SourceVersionKind, StatisticsDepth, StorageType, TableId, TableMeta,
    };
    use kaveon_sql::logical_plan::sql_to_logical_plan;
    use std::{fs, path::PathBuf};
    fn stats(table: &str) -> Option<RelationStatistics> {
        Some(RelationStatistics::exact_with_bytes(
            if table == "small" { 10 } else { 10_000 },
            vec!["id".into(), "value".into()],
            if table == "small" { 1_000 } else { 1_000_000 },
        ))
    }

    #[test]
    fn reads_exact_schema_and_row_count_from_an_iceberg_snapshot() {
        let root =
            std::env::temp_dir().join(format!("kaveon-optim-iceberg-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let metadata = root.join("v2.metadata.json");
        fs::write(
            &metadata,
            serde_json::json!({
                "format-version": 2,
                "location": root.to_string_lossy(),
                "current-snapshot-id": -1,
                "current-schema-id": 1,
                "schemas": [{
                    "type": "struct",
                    "schema-id": 1,
                    "fields": [{"id": 1, "name": "id", "required": true, "type": "long"}]
                }],
                "snapshots": []
            })
            .to_string(),
        )
        .unwrap();
        let resolved = ResolvedTable {
            catalog: "lake".into(),
            schema: "s".into(),
            table: std::sync::Arc::new(TableMeta {
                name: "ice".into(),
                arrow_schema: std::sync::Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
                    "id",
                    arrow::datatypes::DataType::Int64,
                    false,
                )])),
                location: metadata.to_string_lossy().into_owned(),
                access: AccessPattern::Shortcut,
                format: DataFormat::Iceberg,
            }),
            storage: StorageType::Local {
                base_path: PathBuf::new(),
            },
        };
        let statistics = relation_statistics(&resolved).expect("Iceberg metadata is readable");
        assert_eq!(statistics.rows, 0);
        assert_eq!(statistics.columns, vec!["id"]);
        assert_eq!(statistics.bytes_hint, Some(0));
        fs::remove_dir_all(root).unwrap();
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
    fn reorders_a_three_way_chain_to_reduce_the_first_stage_footprint() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM large l JOIN medium m ON l.id=m.id JOIN tiny t ON m.id=t.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut |table| {
            let (rows, bytes) = match table {
                "large" => (10_000_000, 100_000_000),
                "medium" => (100_000, 10_000_000),
                "tiny" => (10, 1_000),
                _ => return None,
            };
            Some(RelationStatistics::exact_with_bytes(
                rows,
                vec!["id".into()],
                bytes,
            ))
        });
        let LogicalPlan::Project { input, .. } = optimized else {
            panic!("projection missing: {optimized:?}")
        };
        let LogicalPlan::Join { left, right, .. } = *input else {
            panic!("reordered outer join missing")
        };
        assert!(matches!(
            right.as_ref(),
            LogicalPlan::Scan { table, .. } if table == "large"
        ));
        let LogicalPlan::Join { left, right, .. } = *left else {
            panic!("small first-stage join missing")
        };
        assert!(matches!(
            left.as_ref(),
            LogicalPlan::Scan { table, .. } if table == "medium"
        ));
        assert!(matches!(
            right.as_ref(),
            LogicalPlan::Scan { table, .. } if table == "tiny"
        ));
    }

    #[test]
    fn reorders_a_four_way_chain_with_exact_output_order() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM large l JOIN medium m ON l.id=m.id JOIN tiny t ON m.id=t.id JOIN micro u ON t.id=u.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut |table| {
            let (rows, bytes) = match table {
                "large" => (10_000_000, 100_000_000),
                "medium" => (100_000, 10_000_000),
                "tiny" => (10, 1_000),
                "micro" => (5, 500),
                _ => return None,
            };
            Some(RelationStatistics::exact_with_bytes(
                rows,
                vec!["id".into()],
                bytes,
            ))
        });
        let LogicalPlan::Project { input, columns } = optimized else {
            panic!("four-way reorder must restore the written projection")
        };
        assert_eq!(columns.len(), 4);
        assert_eq!(
            columns
                .iter()
                .map(|column| match column {
                    Expr::Alias { name, .. } => name.as_str(),
                    _ => panic!("qualified output alias missing"),
                })
                .collect::<Vec<_>>(),
            vec!["l.id", "m.id", "t.id", "u.id"]
        );
        let rendered = format!("{input:?}");
        assert!(rendered.contains("table: \"micro\""));
        assert!(rendered.contains("table: \"tiny\""));
    }

    #[test]
    fn reorders_a_five_way_chain_with_a_connected_left_deep_order() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM large l JOIN medium m ON l.id=m.id JOIN tiny t ON m.id=t.id JOIN micro u ON t.id=u.id JOIN nano n ON u.id=n.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut |table| {
            let (rows, bytes) = match table {
                "large" => (10_000_000, 100_000_000),
                "medium" => (100_000, 10_000_000),
                "tiny" => (10, 1_000),
                "micro" => (5, 500),
                "nano" => (2, 200),
                _ => return None,
            };
            Some(RelationStatistics::exact_with_bytes(
                rows,
                vec!["id".into()],
                bytes,
            ))
        });
        let LogicalPlan::Project { input, columns } = optimized else {
            panic!("five-way reorder must restore the written projection")
        };
        assert_eq!(columns.len(), 5);
        assert_eq!(
            columns
                .iter()
                .map(|column| match column {
                    Expr::Alias { name, .. } => name.as_str(),
                    _ => panic!("qualified output alias missing"),
                })
                .collect::<Vec<_>>(),
            vec!["l.id", "m.id", "t.id", "u.id", "n.id"]
        );
        let rendered = format!("{input:?}");
        assert!(rendered.contains("table: \"nano\""));
        assert!(rendered.contains("table: \"micro\""));
    }

    #[test]
    fn three_way_reordering_is_fail_closed_for_outer_and_using_joins() {
        for sql in [
            "SELECT * FROM large l LEFT JOIN medium m ON l.id=m.id JOIN tiny t ON m.id=t.id",
            "SELECT * FROM large l JOIN medium m USING (id) JOIN tiny t ON m.id=t.id",
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let before = format!("{plan:?}");
            let after = optimize_with_statistics(plan, &mut |table| {
                Some(RelationStatistics::exact_with_bytes(
                    if table == "tiny" { 10 } else { 10_000 },
                    vec!["id".into()],
                    if table == "tiny" { 1_000 } else { 1_000_000 },
                ))
            });
            // Existing binary build-side optimization may still change an
            // inner child; the three-way associativity rewrite itself must not
            // run for these shapes.
            assert!(!format!("{after:?}").contains("__join"));
            assert!(before.contains("Join"));
        }
    }

    #[test]
    fn chooses_the_narrower_inner_build_when_row_counts_disagree() {
        let plan = sql_to_logical_plan("SELECT * FROM small s JOIN big b ON s.id=b.id").unwrap();
        let optimized = optimize_with_statistics(plan, &mut |table| {
            Some(RelationStatistics::exact_with_bytes(
                if table == "small" { 10 } else { 10_000 },
                vec!["id".into(), "value".into()],
                if table == "small" {
                    512 * 1024 * 1024
                } else {
                    8 * 1024 * 1024
                },
            ))
        });
        let LogicalPlan::Join {
            right,
            distribution,
            ..
        } = optimized
        else {
            panic!("join missing: {optimized:?}")
        };
        assert!(matches!(*right, LogicalPlan::Scan { ref table, .. } if table == "big"));
        assert_eq!(distribution, JoinDistribution::Partitioned);
    }

    #[test]
    fn preserves_using_join_metadata_when_statistics_do_not_reorder() {
        let plan = sql_to_logical_plan("SELECT * FROM left_table l JOIN right_table r USING (id)")
            .unwrap();
        let optimized = optimize_with_statistics(plan, &mut |_| {
            Some(RelationStatistics::exact(
                1_000,
                vec!["id".into(), "value".into()],
            ))
        });
        let LogicalPlan::Project { input, .. } = optimized else {
            panic!("USING projection missing")
        };
        let LogicalPlan::Join { using_columns, .. } = *input else {
            panic!("USING join missing")
        };
        assert_eq!(using_columns, Some(vec!["id".to_owned()]));
    }

    #[test]
    fn keeps_exact_stats_through_a_simple_projected_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM big b JOIN (SELECT id, value FROM small) s ON b.id=s.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Join { distribution, .. } = optimized else {
            panic!("join missing: {optimized:?}")
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    #[test]
    fn keeps_exact_stats_through_a_sorted_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM big b JOIN (SELECT id, value FROM small ORDER BY id) s ON b.id=s.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Join { distribution, .. } = optimized else {
            panic!("join missing: {optimized:?}")
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    #[test]
    fn keeps_upper_bound_through_a_distinct_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM big b JOIN (SELECT DISTINCT id FROM small) s ON b.id=s.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Join { distribution, .. } = optimized else {
            panic!("join missing: {optimized:?}")
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    #[test]
    fn keeps_a_limit_bound_through_a_filter_on_a_derived_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM big b JOIN (SELECT id, value FROM small LIMIT 1000) s ON b.id=s.id WHERE s.id > 5",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Filter { input, .. } = optimized else {
            panic!("outer filter missing: {optimized:?}")
        };
        let LogicalPlan::Join { distribution, .. } = *input else {
            panic!("join missing")
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    #[test]
    fn bounds_exact_stats_through_an_offset_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM big b JOIN (SELECT id, value FROM small OFFSET 5) s ON b.id=s.id",
        )
        .unwrap();
        let optimized = optimize_with_statistics(plan, &mut stats);
        let LogicalPlan::Join { distribution, .. } = optimized else {
            panic!("join missing")
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }

    #[test]
    fn keeps_nonzero_byte_upper_bound_through_a_small_limit() {
        let plan = sql_to_logical_plan("SELECT id FROM small LIMIT 1").unwrap();
        let estimate = relation(&plan, &mut |_| {
            Some(RelationStatistics::exact_with_bytes(
                3,
                vec!["id".into()],
                2,
            ))
        })
        .expect("limited scan should retain exact source statistics");

        assert_eq!(estimate.rows, 1);
        // A non-empty limited relation must not become a zero-byte build just
        // because integer division rounded the proportional estimate down.
        assert_eq!(estimate.bytes, Some(1));
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
    fn exact_source_broadcast_bounds_ignore_stale_and_incomplete_documents() {
        let mut source = SourceStatistics {
            identity_sha256: "current".into(),
            row_count: 10,
            bytes: Some(BROADCAST_BUILD_MAX_BYTES + 1),
            columns: vec!["id".into()],
            schema: Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
                "id",
                DataType::Int64,
                false,
            )])),
            delta_version: None,
            parquet_listing: None,
        };
        let mut stored = table_statistics(1);
        stored.bytes = 1;
        let scan = sql_to_logical_plan("SELECT * FROM small WHERE id = 5000").unwrap();
        let stale = RelationStatistics::from_source(&source, Some(Arc::new(stored.clone())));
        assert!(stale.table.is_none());
        let bound = relation(&scan, &mut |_| Some(stale.clone())).unwrap();
        assert_eq!(bound.rows, 10);
        assert_eq!(bound.bytes, source.bytes);
        assert_eq!(
            join_distribution(JoinType::Left, Some(&estimate(10_000, None)), Some(&bound)),
            JoinDistribution::Partitioned,
        );

        stored.source_version.identity_sha256 = source.identity_sha256.clone();
        source.bytes = None;
        let missing = RelationStatistics::from_source(&source, Some(Arc::new(stored.clone())));
        assert!(missing.table.is_some());
        assert_eq!(
            missing.bytes(),
            None,
            "an incomplete document cannot supply zero bytes"
        );
        assert_eq!(
            join_distribution(
                JoinType::Left,
                Some(&estimate(10_000, None)),
                Some(&estimate(missing.rows, missing.bytes())),
            ),
            JoinDistribution::Partitioned,
        );

        source.bytes = Some(1_000);
        stored.bytes = BROADCAST_BUILD_MAX_BYTES + 1;
        let fresh = RelationStatistics::from_source(&source, Some(Arc::new(stored)));
        assert_eq!(fresh.bytes(), Some(1_000));
        assert_eq!(
            join_distribution(
                JoinType::Left,
                Some(&estimate(10_000, None)),
                Some(&estimate(fresh.rows, fresh.bytes())),
            ),
            JoinDistribution::BroadcastRight,
        );
    }

    #[test]
    fn filtered_estimates_never_round_a_nonempty_relation_to_zero() {
        assert_eq!(scaled_upper_bound(1, 0.001), 1);
        assert_eq!(scaled_upper_bound(10, 0.101), 2);
        assert_eq!(scaled_upper_bound(10, 0.0), 0);
        assert_eq!(scaled_upper_bound(0, 1.0), 0);
    }

    #[test]
    fn broadcasts_only_a_proven_small_materially_smaller_inner_build() {
        assert_eq!(
            join_distribution(
                JoinType::Inner,
                Some(&estimate(4_000_000, None)),
                Some(&estimate(1_000_000, Some(BROADCAST_BUILD_MAX_BYTES)))
            ),
            JoinDistribution::BroadcastRight
        );
        for (join_type, probe, build) in [
            (JoinType::Inner, 4_000_000, Some(1_000_000)),
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
    fn broadcasts_a_proven_small_right_side_of_a_left_join() {
        assert_eq!(
            join_distribution(
                JoinType::Left,
                Some(&estimate(4_000_000, Some(128 * 1024 * 1024))),
                Some(&estimate(1_000, Some(8 * 1024 * 1024))),
            ),
            JoinDistribution::BroadcastRight
        );
        assert_eq!(
            join_distribution(
                JoinType::Left,
                Some(&estimate(4_000_000, None)),
                Some(&estimate(1_000, Some(BROADCAST_BUILD_MAX_BYTES + 1))),
            ),
            JoinDistribution::Partitioned
        );
    }

    #[test]
    fn exact_source_byte_hints_reject_an_oversized_broadcast_build() {
        let plan = sql_to_logical_plan("SELECT * FROM big b JOIN small s ON b.id=s.id").unwrap();
        let optimized = optimize_with_statistics(plan, &mut |table| {
            Some(RelationStatistics::exact_with_bytes(
                if table == "small" { 10 } else { 10_000 },
                vec!["id".into()],
                if table == "small" {
                    BROADCAST_BUILD_MAX_BYTES + 1
                } else {
                    1_000
                },
            ))
        });
        let LogicalPlan::Project { input, .. } = optimized else {
            panic!("reordered join projection missing")
        };
        let LogicalPlan::Join { distribution, .. } = *input else {
            panic!("join missing")
        };
        assert_eq!(distribution, JoinDistribution::Partitioned);
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
                bytes_hint: Some(100_000_000),
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
        assert!(matches!(
            left,
            LogicalPlan::Filter { input, .. }
                if matches!(input.as_ref(), LogicalPlan::Scan { table, .. } if table == "other")
        ));
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

    #[test]
    fn a_limited_build_relation_uses_its_cardinality_bound_for_broadcast() {
        let plan = LogicalPlan::Join {
            left: Box::new(LogicalPlan::Scan {
                table: "large".into(),
                alias: Some("l".into()),
                columns: None,
            }),
            right: Box::new(LogicalPlan::Limit {
                input: Box::new(LogicalPlan::Scan {
                    table: "lookup".into(),
                    alias: Some("r".into()),
                    columns: None,
                }),
                count: 100,
            }),
            join_type: JoinType::Inner,
            condition: Some(Expr::BinaryOp {
                left: Box::new(Expr::Column("l.id".into())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Column("r.id".into())),
            }),
            using_columns: None,
            distribution: JoinDistribution::Partitioned,
        };
        let optimized = optimize_with_statistics(plan, &mut |table| {
            Some(RelationStatistics::exact_with_bytes(
                if table == "large" {
                    10_000_000
                } else {
                    1_000_000
                },
                vec!["id".into()],
                1_000_000,
            ))
        });
        let LogicalPlan::Join { distribution, .. } = optimized else {
            panic!("join was unexpectedly rewritten");
        };
        assert_eq!(distribution, JoinDistribution::BroadcastRight);
    }
}
