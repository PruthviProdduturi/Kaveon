//! Binding: the pass that gives the logical plan what the SQL text left
//! implicit — which relation each bare column belongs to.
//!
//! The SQL layer lowers `FROM customer, orders WHERE c_custkey = o_custkey`
//! to a cross join under a filter, because without the catalog it cannot
//! tell which side `c_custkey` lives on. With the catalog every scan has a
//! scope (its columns, under its alias or table name), and every join's
//! scope is its inputs' scopes qualified the way the join operator names
//! its output (`customer.c_custkey`). Against those scopes this pass:
//!
//! - routes the conjuncts of a filter over a join tree: a conjunct on one
//!   side moves to that side, an equality between the two sides becomes a
//!   hash-join key (the cross join becomes an inner join), and the rest
//!   stays above the join;
//! - qualifies bare column references under a join with their relation,
//!   so the qualifier-driven passes (filter pushdown, projection pruning,
//!   build-side choice) and the join operator's output names agree, while
//!   a projected column keeps the name it was written with.
//!
//! Above a single scan, a projection or an aggregate nothing is rewritten:
//! the operators resolve bare names against their inputs themselves. A
//! table the catalog cannot describe leaves its references as written for
//! the planner to report.
//!
//! A subquery binds with the enclosing query's scope behind its own. A
//! WHERE conjunct that references the enclosing query is a correlation:
//! it leaves the subquery's filter and rides up through the subquery's
//! projection (as extra columns) to the node that joins the subquery in.
//! An equality between one column of the subquery and one of the
//! enclosing query becomes that join's key — `EXISTS (... WHERE
//! l_orderkey = o_orderkey)` is a semi join on the key, and `x < (SELECT
//! avg(y) FROM t WHERE t.k = q.k)` is an inner join on `k` with the
//! aggregate grouped by `k` (the correlation rides through the aggregate
//! as a group key), which keeps exactly the rows whose comparison with
//! the per-key aggregate can hold. Any other conjunct — `l2.l_suppkey <>
//! l1.l_suppkey` beside the equality in Q21's EXISTS — is a residual of
//! the semi (or anti) join: evaluated over each pair of rows sharing the
//! key, with the subquery's side of it projected out beside the key.
//! Residuals ride only through projections, filters and sorts; under an
//! aggregate, a join or an IN they are refused with their text.
use std::cell::Cell;

use kaveon_core::{
    BinaryOp, CatalogManager, Expr, KaveonError, Result, TableReference, expression_contains_column,
};
use kaveon_sql::logical_plan::{AggregateExpr, JoinType, LogicalPlan};

/// Bind `plan` against `catalog`. Table names must already be qualified.
///
/// Every scanned table must resolve in `catalog`: this is where a
/// statement's references are checked against what the session may see,
/// so a catalog the session was not granted — presented as absent by a
/// restricted `CatalogManager` — fails here as `catalog 'x' not found`,
/// before planning, exactly as a catalog that does not exist.
pub fn bind(plan: LogicalPlan, catalog: &CatalogManager) -> Result<LogicalPlan> {
    Binder {
        catalog,
        names: Cell::new(0),
    }
    .bind(plan, &[])
    .map(|bound| bound.plan)
}

/// One output column of a plan node: how a query may refer to it, and the
/// name it carries in the operator's output.
#[derive(Clone, Debug, PartialEq)]
struct Column {
    qualifier: Option<String>,
    name: String,
    physical: String,
}

/// The output columns of a plan node.
#[derive(Clone, Debug, Default)]
struct Scope {
    columns: Vec<Column>,
    /// The output of a join: bare references are rewritten to their
    /// physical (qualified) names so the qualifier-driven passes see them.
    joined: bool,
}

enum Resolution {
    Unresolved,
    Unique(String),
    Ambiguous,
}

impl Scope {
    fn resolve(&self, reference: &str) -> Resolution {
        let mut physical: Vec<&str> = match reference.rsplit_once('.') {
            Some((qualifier, name)) => self
                .columns
                .iter()
                .filter(|column| {
                    column.qualifier.as_deref() == Some(qualifier) && column.name == name
                })
                .map(|column| column.physical.as_str())
                .collect(),
            None => self
                .columns
                .iter()
                .filter(|column| column.name == reference)
                .map(|column| column.physical.as_str())
                .collect(),
        };
        physical.sort_unstable();
        physical.dedup();
        match physical.as_slice() {
            [] => Resolution::Unresolved,
            [one] => Resolution::Unique((*one).to_owned()),
            _ => Resolution::Ambiguous,
        }
    }

    fn holds(&self, reference: &str) -> bool {
        matches!(self.resolve(reference), Resolution::Unique(_))
    }

    /// The columns as a join names them: qualified with `qualifier` when
    /// the input is a relation the join qualifies.
    fn qualified_by(mut self, qualifier: Option<&str>) -> Self {
        if let Some(qualifier) = qualifier {
            for column in &mut self.columns {
                column.physical = format!("{qualifier}.{}", column.physical);
            }
        }
        self
    }
}

struct Bound {
    plan: LogicalPlan,
    /// The input scope of the aggregate this node sits on, so aggregate
    /// function references above it (`SUM(l_quantity)` in the projection
    /// or HAVING) bind the way the aggregate bound its arguments.
    aggregate_input: Option<Scope>,
    /// Conjuncts referencing the enclosing query lifted out of a
    /// subquery's WHERE, on their way to the node that joins the subquery
    /// in.
    correlations: Vec<Correlation>,
}

impl Bound {
    fn plain(plan: LogicalPlan) -> Self {
        Self {
            plan,
            aggregate_input: None,
            correlations: Vec::new(),
        }
    }
}

/// A conjunct lifted out of a subquery's WHERE because it references the
/// enclosing query. `depth` is the enclosing scope the reference resolved
/// in (an index into the scope stack; the innermost is last).
#[derive(Clone, Debug)]
enum Correlation {
    /// `inner = outer`: `inner` names the subquery-side column at the
    /// current node's output, `outer` the enclosing query's column as
    /// written.
    Key {
        inner: String,
        outer: String,
        depth: usize,
    },
    /// Any other conjunct: `predicate` with the subquery's side bound
    /// (and renamed as it rides up) and the enclosing query's side as
    /// written; `inner` the subquery-side columns it reads, at the
    /// current node's output.
    Residual {
        predicate: Expr,
        inner: Vec<String>,
        depth: usize,
    },
}

impl Correlation {
    fn depth(&self) -> usize {
        match self {
            Self::Key { depth, .. } | Self::Residual { depth, .. } => *depth,
        }
    }
}

/// A bound semi or anti join: its inputs, keys and residual, and the
/// correlations its left input carries upward.
struct BoundSemiJoin {
    left: Box<LogicalPlan>,
    right: Box<LogicalPlan>,
    left_key: Expr,
    right_key: Expr,
    residual: Option<Expr>,
    correlations: Vec<Correlation>,
}

/// Where a conjunct of a filter over a join belongs.
enum Placement {
    Left,
    Right,
    Key(String, String),
    Above,
}

struct Binder<'a> {
    catalog: &'a CatalogManager,
    /// Names for the columns a correlation rides along under.
    names: Cell<usize>,
}

impl Binder<'_> {
    fn bind(&self, plan: LogicalPlan, outer: &[Scope]) -> Result<Bound> {
        match plan {
            LogicalPlan::Scan { ref table, .. } => {
                self.catalog.resolve_table(&TableReference::parse(table))?;
                Ok(Bound::plain(plan))
            }
            LogicalPlan::Unnest {
                ref expressions, ..
            } => {
                if expressions.iter().any(expression_contains_column) {
                    return Err(KaveonError::Sql(
                        "correlated UNNEST requires CROSS JOIN LATERAL".into(),
                    ));
                }
                Ok(Bound::plain(plan))
            }
            LogicalPlan::Filter { input, predicate } => {
                let input = self.bind(*input, outer)?;
                let scope = self.scope_of(&input.plan);
                let mut correlations = input.correlations;
                let mut local = Vec::new();
                for conjunct in conjuncts(predicate) {
                    match self.correlation(&conjunct, &scope, outer)? {
                        Some(correlation) => correlations.push(correlation),
                        None => local.push(conjunct),
                    }
                }
                let (plan, residual) = self.route(local, input.plan)?;
                let scope = self.scope_of(&plan);
                let plan = self.filtered(plan, residual, &scope, input.aggregate_input.as_ref())?;
                Ok(Bound {
                    plan,
                    aggregate_input: input.aggregate_input,
                    correlations,
                })
            }
            LogicalPlan::Project { input, columns } => {
                let input = self.bind(*input, outer)?;
                let scope = self.scope_of(&input.plan);
                let mut columns = columns
                    .into_iter()
                    .flat_map(|column| match column {
                        Expr::Star => self
                            .using_wildcard_columns(&input.plan)
                            .unwrap_or_else(|| vec![Expr::Star]),
                        Expr::Column(name) if name.ends_with(".*") => self
                            .qualified_wildcard_columns(&input.plan, name.trim_end_matches(".*"))
                            .unwrap_or_else(|| vec![Expr::Column(name)]),
                        other => vec![other],
                    })
                    .map(|column| {
                        self.bind_projected(column, &scope, input.aggregate_input.as_ref())
                    })
                    .collect::<Result<Vec<_>>>()?;
                // A correlated column rides through the projection under a
                // name of its own; a residual's predicate follows the
                // rename.
                let correlations = input
                    .correlations
                    .into_iter()
                    .map(|correlation| {
                        let mut project = |inner: String| {
                            let name = self.fresh_name();
                            columns.push(Expr::Alias {
                                expr: Box::new(Expr::Column(inner)),
                                name: name.clone(),
                            });
                            name
                        };
                        match correlation {
                            Correlation::Key {
                                inner,
                                outer,
                                depth,
                            } => Correlation::Key {
                                inner: project(inner),
                                outer,
                                depth,
                            },
                            Correlation::Residual {
                                mut predicate,
                                inner,
                                depth,
                            } => {
                                let inner = inner
                                    .into_iter()
                                    .map(|column| {
                                        let name = project(column.clone());
                                        predicate =
                                            rename_column(predicate.clone(), &column, &name);
                                        name
                                    })
                                    .collect();
                                Correlation::Residual {
                                    predicate,
                                    inner,
                                    depth,
                                }
                            }
                        }
                    })
                    .collect();
                Ok(Bound {
                    plan: LogicalPlan::Project {
                        input: Box::new(input.plan),
                        columns,
                    },
                    aggregate_input: None,
                    correlations,
                })
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.bind(*input, outer)?;
                let scope = self.scope_of(&input.plan);
                let mut group_by = group_by
                    .into_iter()
                    .map(|key| self.bind_expr(key, &scope, None))
                    .collect::<Result<Vec<_>>>()?;
                let aggregates = aggregates
                    .into_iter()
                    .map(|aggregate| self.bind_aggregate(aggregate, &scope))
                    .collect::<Result<Vec<_>>>()?;
                // A correlated column becomes a group key: the aggregate
                // answers per value of the enclosing query's column, and
                // the join above selects the row for its value. A residual
                // selects the rows the aggregate sees per enclosing row,
                // which no grouping can express.
                if !input.correlations.is_empty() {
                    for correlation in &input.correlations {
                        let inner = match correlation {
                            Correlation::Key { inner, .. } => inner,
                            Correlation::Residual { predicate, .. } => {
                                return Err(KaveonError::Sql(format!(
                                    "a correlated predicate other than an equality is not supported under an aggregate: {predicate:?}"
                                )));
                            }
                        };
                        let key = Expr::Column(inner.clone());
                        if !group_by.contains(&key) {
                            group_by.push(key);
                        }
                    }
                }
                Ok(Bound {
                    plan: LogicalPlan::Aggregate {
                        input: Box::new(input.plan),
                        group_by,
                        aggregates,
                    },
                    aggregate_input: Some(scope),
                    correlations: input.correlations,
                })
            }
            LogicalPlan::Sort { input, order_by } => {
                let input = self.bind(*input, outer)?;
                let scope = self.scope_of(&input.plan);
                let order_by = order_by
                    .into_iter()
                    .map(|(key, ascending, nulls_first)| {
                        // `sql_to_logical_plan` resolves ordinal ORDER BY
                        // keys while the projection still contains `*`.
                        // A USING join expands that wildcard during binding,
                        // so resolve any ordinal that survived lowering
                        // against the now-bound output scope. Leaving it as
                        // a literal would make every row tie and lets local
                        // and distributed sort paths choose different rows
                        // under LIMIT.
                        let key = match key {
                            Expr::Literal(kaveon_core::ScalarValue::Int64(ordinal)) => {
                                let position = ordinal
                                    .checked_sub(1)
                                    .and_then(|value| usize::try_from(value).ok())
                                    .ok_or_else(|| {
                                        KaveonError::Sql(
                                            "ORDER BY position must be greater than zero"
                                                .to_owned(),
                                        )
                                    })?;
                                let column = scope.columns.get(position).ok_or_else(|| {
                                    KaveonError::Sql(format!(
                                        "ORDER BY position {ordinal} is not in the select list"
                                    ))
                                })?;
                                Expr::Column(column.physical.clone())
                            }
                            key => key,
                        };
                        self.bind_expr(key, &scope, input.aggregate_input.as_ref())
                            .map(|key| (key, ascending, nulls_first))
                    })
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Sort {
                        input: Box::new(input.plan),
                        order_by,
                    },
                    aggregate_input: input.aggregate_input,
                    correlations: input.correlations,
                })
            }
            LogicalPlan::Limit { input, count } => {
                let input = self.bind(*input, outer)?;
                // A scalar correlated OFFSET ... LIMIT 1 is decorrelated in
                // the enclosing Join by ranking rows per equality key. Keep
                // the bounded limit's correlations alive long enough for
                // that rewrite; every other correlated paging shape is
                // rejected by the Join path below rather than executing a
                // globally-scoped limit by accident.
                if !input.correlations.is_empty() && count != 1 {
                    uncorrelated(&input, "LIMIT")?;
                }
                Ok(Bound {
                    plan: LogicalPlan::Limit {
                        input: Box::new(input.plan),
                        count,
                    },
                    ..input
                })
            }
            LogicalPlan::Offset { input, count } => {
                let input = self.bind(*input, outer)?;
                // OFFSET 0 cannot change a relation, including a correlated
                // scalar subquery.  Removing it before correlation checks
                // keeps the safe no-op case decorrelatable while positive
                // offsets still fail closed below.
                if count == 0 {
                    return Ok(input);
                }
                Ok(Bound {
                    plan: LogicalPlan::Offset {
                        input: Box::new(input.plan),
                        count,
                    },
                    ..input
                })
            }
            LogicalPlan::Distinct { input } => {
                let input = self.bind(*input, outer)?;
                Ok(Bound {
                    plan: LogicalPlan::Distinct {
                        input: Box::new(input.plan),
                    },
                    ..input
                })
            }
            LogicalPlan::Window {
                input,
                window_exprs,
            } => {
                let input = self.bind(*input, outer)?;
                let scope = self.scope_of(&input.plan);
                let window_exprs = window_exprs
                    .into_iter()
                    .map(|expr| self.bind_expr(expr, &scope, input.aggregate_input.as_ref()))
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Window {
                        input: Box::new(input.plan),
                        window_exprs,
                    },
                    ..input
                })
            }
            LogicalPlan::Union { inputs, all } => {
                let inputs = inputs
                    .into_iter()
                    .map(|input| {
                        let input = self.bind(input, outer)?;
                        uncorrelated(&input, "UNION")?;
                        Ok(input.plan)
                    })
                    .collect::<Result<_>>()?;
                Ok(Bound::plain(LogicalPlan::Union { inputs, all }))
            }
            LogicalPlan::Intersect { left, right, all } => {
                let left = self.bind(*left, outer)?;
                let right = self.bind(*right, outer)?;
                uncorrelated(&left, "INTERSECT")?;
                uncorrelated(&right, "INTERSECT")?;
                Ok(Bound::plain(LogicalPlan::Intersect {
                    left: Box::new(left.plan),
                    right: Box::new(right.plan),
                    all,
                }))
            }
            LogicalPlan::Except { left, right, all } => {
                let left = self.bind(*left, outer)?;
                let right = self.bind(*right, outer)?;
                uncorrelated(&left, "EXCEPT")?;
                uncorrelated(&right, "EXCEPT")?;
                Ok(Bound::plain(LogicalPlan::Except {
                    left: Box::new(left.plan),
                    right: Box::new(right.plan),
                    all,
                }))
            }
            LogicalPlan::Join {
                left,
                right,
                join_type,
                condition,
                using_columns,
                distribution,
            } => {
                let left = self.bind(*left, outer)?;
                // The right input may reference the left (a scalar subquery
                // correlated with the query it sits in): it binds with the
                // left's scope as its innermost enclosing scope, and such
                // correlations become this join's keys.
                let mut right_outer = outer.to_vec();
                right_outer.push(self.scope_of(&left.plan));
                let lateral_unnest = matches!(
                    right.as_ref(),
                    LogicalPlan::Unnest { expressions, .. }
                        if expressions.iter().any(expression_contains_column)
                );
                let mut right = if lateral_unnest {
                    Bound::plain(*right)
                } else {
                    self.bind(*right, &right_outer)?
                };
                // A non-constant UNNEST on the right of a CROSS JOIN is a
                // lateral table function. Bind its expressions against the
                // complete left scope; the physical planner will execute the
                // resulting operator once per left input batch.
                if join_type == JoinType::Cross
                    && condition.is_none()
                    && let LogicalPlan::Unnest {
                        expressions,
                        columns,
                        with_ordinality,
                        ordinality_column,
                        ordinality_start,
                    } = &right.plan
                    && expressions.iter().any(expression_contains_column)
                {
                    let left_scope = self.scope_of(&left.plan);
                    let expressions = expressions
                        .iter()
                        .cloned()
                        .map(|expression| self.bind_expr(expression, &left_scope, None))
                        .collect::<Result<Vec<_>>>()?;
                    return Ok(Bound {
                        plan: LogicalPlan::Join {
                            left: Box::new(left.plan),
                            right: Box::new(LogicalPlan::Unnest {
                                expressions,
                                columns: columns.clone(),
                                with_ordinality: *with_ordinality,
                                ordinality_column: ordinality_column.clone(),
                                ordinality_start: *ordinality_start,
                            }),
                            join_type,
                            condition,
                            using_columns,
                            distribution,
                        },
                        aggregate_input: None,
                        correlations: left.correlations,
                    });
                }
                let (lateral, deeper): (Vec<_>, Vec<_>) = right
                    .correlations
                    .into_iter()
                    .partition(|correlation| correlation.depth() == outer.len());
                let mut correlations = left.correlations;
                correlations.extend(deeper);
                if !lateral.is_empty() {
                    let key_names = lateral
                        .iter()
                        .map(|correlation| match correlation {
                            Correlation::Key { inner, .. } => Ok(inner.clone()),
                            Correlation::Residual { .. } => Err(KaveonError::Sql(
                                "a positive correlated scalar OFFSET requires equality correlations only; OFFSET inside a correlated subquery is not supported for residual predicates"
                                    .into(),
                            )),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if let Some((mut plan, offset, limit, order_by)) =
                        peel_scalar_paging(right.plan.clone())
                    {
                        if offset > 0 && limit == 1 {
                            // The scalar projection hides the sort keys. Carry
                            // each key through that projection before the
                            // per-key window evaluates it; otherwise the
                            // window would try to resolve the original inner
                            // qualifier against only the scalar output.
                            let mut window_order = Vec::with_capacity(order_by.len());
                            if let LogicalPlan::Project { columns, .. } = &mut plan {
                                for (expression, ascending, nulls_first) in order_by {
                                    let name = self.fresh_name();
                                    columns.push(Expr::Alias {
                                        expr: Box::new(expression),
                                        name: name.clone(),
                                    });
                                    window_order.push((Expr::Column(name), ascending, nulls_first));
                                }
                            } else if !order_by.is_empty() {
                                return Err(KaveonError::Sql(
                                    "correlated scalar OFFSET requires a projected order key"
                                        .into(),
                                ));
                            }
                            let rank = Expr::WindowFunction {
                                name: "ROW_NUMBER".into(),
                                args: Vec::new(),
                                partition_by: key_names
                                    .iter()
                                    .map(|name| Expr::Column(name.clone()))
                                    .collect(),
                                order_by: window_order,
                                frame: None,
                            };
                            let upper = i64::try_from(offset)
                                .ok()
                                .and_then(|value| value.checked_add(1))
                                .ok_or_else(|| {
                                    KaveonError::Sql("correlated scalar OFFSET is too large".into())
                                })?;
                            let rank_filter = Expr::And(
                                Box::new(Expr::BinaryOp {
                                    left: Box::new(rank.clone()),
                                    op: BinaryOp::Gt,
                                    right: Box::new(Expr::Literal(
                                        kaveon_core::ScalarValue::Int64(
                                            i64::try_from(offset).map_err(|_| {
                                                KaveonError::Sql(
                                                    "correlated scalar OFFSET is too large".into(),
                                                )
                                            })?,
                                        ),
                                    )),
                                }),
                                Box::new(Expr::BinaryOp {
                                    left: Box::new(rank.clone()),
                                    op: BinaryOp::Le,
                                    right: Box::new(Expr::Literal(
                                        kaveon_core::ScalarValue::Int64(upper),
                                    )),
                                }),
                            );
                            right.plan = LogicalPlan::Filter {
                                input: Box::new(LogicalPlan::Window {
                                    input: Box::new(plan),
                                    window_exprs: vec![rank.clone()],
                                }),
                                predicate: rank_filter,
                            };
                        } else {
                            return Err(KaveonError::Sql(
                                "correlated paging supports only OFFSET n LIMIT 1".into(),
                            ));
                        }
                    } else if contains_paging(&right.plan) {
                        return Err(KaveonError::Sql(
                            "correlated paging supports only scalar OFFSET n LIMIT 1".into(),
                        ));
                    }
                }
                // A correlated COUNT needs to retain an outer row when its
                // grouped build side has no matching key. The aggregate
                // emits no group for that key, so use a left join and turn
                // the null-extended count into SQL's required zero.
                let correlated_count = !lateral.is_empty() && contains_count_aggregate(&right.plan);
                if correlated_count {
                    right.plan = coalesce_count_projection(right.plan);
                }
                let mut keys = Vec::new();
                let mut residuals = Vec::new();
                for correlation in lateral {
                    match correlation {
                        Correlation::Key { inner, outer, .. } => keys.push(Expr::BinaryOp {
                            left: Box::new(Expr::Column(outer)),
                            op: BinaryOp::Eq,
                            right: Box::new(Expr::Column(inner)),
                        }),
                        Correlation::Residual { predicate, .. } => residuals.push(predicate),
                    }
                }
                if !residuals.is_empty()
                    && !matches!(
                        join_type,
                        JoinType::Inner | JoinType::Left | JoinType::Cross
                    )
                {
                    return Err(KaveonError::Sql(
                        "residual correlated predicates are supported only for inner or left lateral joins".into(),
                    ));
                }
                let allow_lateral_outer_residual =
                    join_type == JoinType::Left && !residuals.is_empty();
                let condition =
                    conjoin(condition.into_iter().chain(keys).chain(residuals).collect());
                let Some(condition) = condition else {
                    return Ok(Bound {
                        plan: LogicalPlan::Join {
                            left: Box::new(left.plan),
                            right: Box::new(right.plan),
                            join_type,
                            condition: None,
                            using_columns,
                            distribution,
                        },
                        aggregate_input: None,
                        correlations,
                    });
                };
                let join_type = if correlated_count && join_type == JoinType::Cross {
                    JoinType::Left
                } else {
                    join_type
                };
                let mut bound = self.bind_on(
                    left.plan,
                    right.plan,
                    join_type,
                    condition,
                    using_columns,
                    distribution,
                    allow_lateral_outer_residual,
                )?;
                bound.correlations = correlations;
                Ok(bound)
            }
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            } => {
                let join = self
                    .bind_semi_join(*left, *right, left_key, right_key, residual, false, outer)?;
                Ok(Bound {
                    plan: LogicalPlan::SemiJoin {
                        left: join.left,
                        right: join.right,
                        left_key: join.left_key,
                        right_key: join.right_key,
                        residual: join.residual,
                    },
                    aggregate_input: None,
                    correlations: join.correlations,
                })
            }
            LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            } => {
                let join =
                    self.bind_semi_join(*left, *right, left_key, right_key, residual, true, outer)?;
                Ok(Bound {
                    plan: LogicalPlan::AntiJoin {
                        left: join.left,
                        right: join.right,
                        left_key: join.left_key,
                        right_key: join.right_key,
                        residual: join.residual,
                    },
                    aggregate_input: None,
                    correlations: join.correlations,
                })
            }
        }
    }

    /// Expand an unqualified wildcard over a USING join. SQL exposes the
    /// join key once, using the left relation's position for inner/left
    /// joins, the right value for a right join, and COALESCE for a full join;
    /// an ON join keeps both columns because it has no `using_columns`
    /// metadata.
    fn using_wildcard_columns(&self, plan: &LogicalPlan) -> Option<Vec<Expr>> {
        let LogicalPlan::Join {
            left,
            right,
            using_columns: Some(using_columns),
            ..
        } = plan
        else {
            return None;
        };
        let left = self.scope_of(left).qualified_by(relation_qualifier(left));
        let right = self.scope_of(right).qualified_by(relation_qualifier(right));
        let join_type = match plan {
            LogicalPlan::Join { join_type, .. } => *join_type,
            _ => unreachable!("using_wildcard_columns matched a join"),
        };
        let mut columns = left
            .columns
            .into_iter()
            .map(|column| {
                if !using_columns.iter().any(|name| name == &column.name) {
                    return Expr::Column(column.physical);
                }
                let right_key = right
                    .columns
                    .iter()
                    .find(|candidate| candidate.name == column.name)
                    .map(|candidate| Expr::Column(candidate.physical.clone()));
                match (join_type, right_key) {
                    (JoinType::Right, Some(right_key)) => right_key,
                    (JoinType::Full, Some(right_key)) => Expr::Function {
                        name: "COALESCE".to_owned(),
                        args: vec![Expr::Column(column.physical), right_key],
                    },
                    _ => Expr::Column(column.physical),
                }
            })
            .collect::<Vec<_>>();
        columns.extend(
            right
                .columns
                .into_iter()
                .filter(|column| !using_columns.iter().any(|name| name == &column.name))
                .map(|column| Expr::Column(column.physical)),
        );
        Some(columns)
    }

    fn qualified_wildcard_columns(&self, plan: &LogicalPlan, qualifier: &str) -> Option<Vec<Expr>> {
        let scope = self.scope_of(plan);
        let columns = scope
            .columns
            .into_iter()
            .filter(|column| column.qualifier.as_deref() == Some(qualifier))
            .map(|column| Expr::Column(column.physical))
            .collect::<Vec<_>>();
        (!columns.is_empty()).then_some(columns)
    }

    fn fresh_name(&self) -> String {
        let ordinal = self.names.get();
        self.names.set(ordinal + 1);
        format!("__kaveon_corr_{ordinal}")
    }

    /// The correlation a WHERE conjunct of a subquery expresses, if it
    /// references the enclosing query: an equality between one column of
    /// the subquery and one of the enclosing query is a key; any other
    /// conjunct is a residual, whose every column must belong to the
    /// subquery or to one enclosing query. A conjunct over an aggregate
    /// (a HAVING) is not a row predicate and is refused.
    fn correlation(
        &self,
        conjunct: &Expr,
        scope: &Scope,
        outer: &[Scope],
    ) -> Result<Option<Correlation>> {
        // An aggregate's argument (HAVING sum(x) > 300) is the aggregate's
        // input, not a reference the enclosing query could satisfy.
        let mut references = Vec::new();
        free_column_references(conjunct, &mut references);
        let enclosing = |reference: &str| {
            matches!(scope.resolve(reference), Resolution::Unresolved)
                .then(|| outer.iter().rposition(|scope| scope.holds(reference)))
                .flatten()
        };
        // Inside a subquery, a qualified reference that names neither the
        // subquery's relations nor the enclosing query's is refused: an
        // executor would resolve it by bare name against the subquery's
        // own relation and answer as if it were uncorrelated.
        if !outer.is_empty() && !scope.columns.is_empty() {
            for reference in &references {
                if reference.contains('.')
                    && matches!(scope.resolve(reference), Resolution::Unresolved)
                    && enclosing(reference).is_none()
                {
                    return Err(KaveonError::Sql(format!(
                        "column '{reference}' belongs neither to the subquery nor to the enclosing query: {conjunct:?}"
                    )));
                }
            }
        }
        let depths = references
            .iter()
            .filter_map(|reference| enclosing(reference))
            .collect::<Vec<_>>();
        let Some(&depth) = depths.first() else {
            return Ok(None);
        };
        if let Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } = conjunct
            && let (Expr::Column(a), Expr::Column(b)) = (left.as_ref(), right.as_ref())
        {
            let (inner, outer_column) = if scope.holds(a) { (a, b) } else { (b, a) };
            if let (true, Some(depth)) = (scope.holds(inner), enclosing(outer_column)) {
                return Ok(Some(Correlation::Key {
                    inner: self.bind_name(inner, scope)?,
                    outer: outer_column.clone(),
                    depth,
                }));
            }
        }
        if contains_aggregate(conjunct) {
            return Err(KaveonError::Sql(format!(
                "a correlated HAVING predicate is not supported: {conjunct:?}"
            )));
        }
        if depths.iter().any(|candidate| *candidate != depth) {
            return Err(KaveonError::Sql(format!(
                "a correlated predicate referencing more than one enclosing query is not supported: {conjunct:?}"
            )));
        }
        let mut inner = Vec::new();
        for reference in &references {
            match scope.resolve(reference) {
                Resolution::Unique(_) => inner.push(self.bind_name(reference, scope)?),
                Resolution::Ambiguous => {
                    return Err(KaveonError::Sql(format!(
                        "column '{reference}' is ambiguous"
                    )));
                }
                Resolution::Unresolved if enclosing(reference).is_none() => {
                    return Err(KaveonError::Sql(format!(
                        "column '{reference}' belongs neither to the subquery nor to the enclosing query: {conjunct:?}"
                    )));
                }
                Resolution::Unresolved => {}
            }
        }
        inner.sort_unstable();
        inner.dedup();
        Ok(Some(Correlation::Residual {
            predicate: self.bind_expr(conjunct.clone(), scope, None)?,
            inner,
            depth,
        }))
    }

    /// An explicit ON condition: equalities between the two sides are the
    /// hash-join keys. A conjunct on one side alone filters that side
    /// before the join when the join can drop that side's rows (either
    /// side of an inner join, the non-preserved side of an outer join): a
    /// right row failing `o_comment NOT LIKE ...` matches no left row
    /// under a LEFT JOIN, so filtering it away first is the same join. An
    /// inner join filters anything else above itself; an outer join has no
    /// residual and refuses it.
    #[allow(clippy::too_many_arguments)]
    fn bind_on(
        &self,
        left: LogicalPlan,
        right: LogicalPlan,
        join_type: JoinType,
        condition: Expr,
        using_columns: Option<Vec<String>>,
        distribution: kaveon_sql::logical_plan::JoinDistribution,
        allow_lateral_outer_residual: bool,
    ) -> Result<Bound> {
        let left_scope = self.scope_of(&left).qualified_by(relation_qualifier(&left));
        let right_scope = self
            .scope_of(&right)
            .qualified_by(relation_qualifier(&right));
        let (into_left, into_right) = match join_type {
            JoinType::Inner | JoinType::Cross => (true, true),
            JoinType::Left => (false, true),
            JoinType::Right => (true, false),
            JoinType::Full => (false, false),
        };
        let mut left_parts = Vec::new();
        let mut right_parts = Vec::new();
        let mut keys = Vec::new();
        let mut above = Vec::new();
        for conjunct in conjuncts(condition) {
            match placement(&conjunct, &left_scope, &right_scope) {
                Placement::Key(left, right) => keys.push(Expr::BinaryOp {
                    left: Box::new(Expr::Column(left)),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Column(right)),
                }),
                Placement::Left if into_left => left_parts.push(conjunct),
                Placement::Right if into_right => right_parts.push(conjunct),
                _ => above.push(conjunct),
            }
        }
        if !above.is_empty()
            && !matches!(join_type, JoinType::Inner | JoinType::Cross)
            && !(allow_lateral_outer_residual && join_type == JoinType::Left)
        {
            return Err(KaveonError::Sql(format!(
                "{} JOIN conditions other than equalities between the two sides are not supported: {:?}",
                match join_type {
                    JoinType::Left => "LEFT",
                    JoinType::Right => "RIGHT",
                    _ => "FULL",
                },
                above[0]
            )));
        }
        let left = self.place(left_parts, left)?;
        let right = self.place(right_parts, right)?;
        let join_type = if keys.is_empty() {
            join_type
        } else if join_type == JoinType::Cross {
            JoinType::Inner
        } else {
            join_type
        };
        let join_condition = if allow_lateral_outer_residual && join_type == JoinType::Left {
            conjoin(keys.into_iter().chain(above.clone()).collect())
        } else {
            conjoin(keys)
        };
        let plan = LogicalPlan::Join {
            left: Box::new(left),
            right: Box::new(right),
            join_type,
            condition: join_condition,
            using_columns,
            distribution,
        };
        let scope = self.scope_of(&plan);
        let plan = self.filtered(
            plan,
            if allow_lateral_outer_residual && join_type == JoinType::Left {
                Vec::new()
            } else {
                above
            },
            &scope,
            None,
        )?;
        Ok(Bound::plain(plan))
    }

    /// IN and EXISTS subqueries. A correlated EXISTS becomes a semi (or
    /// anti) join keyed on its first equality with the enclosing query:
    /// the subquery projects its side of the equality, and the enclosing
    /// query's side is the probe key. Its other correlations — further
    /// equalities, and any other conjunct — are the join's residual,
    /// evaluated over the pairs sharing the key; the subquery projects
    /// the columns the residual reads beside the key, under names of
    /// their own, and the residual's enclosing-query side binds against
    /// the left input. NOT EXISTS matches nothing on a NULL key, so the
    /// anti join's build side drops NULL keys first (NOT IN, which the
    /// same operator serves, keeps them: a NULL there empties the
    /// result).
    #[allow(clippy::too_many_arguments)]
    fn bind_semi_join(
        &self,
        left: LogicalPlan,
        right: LogicalPlan,
        left_key: Expr,
        right_key: Expr,
        residual: Option<Expr>,
        anti: bool,
        outer: &[Scope],
    ) -> Result<BoundSemiJoin> {
        let left = self.bind(left, outer)?;
        let left_scope = self.scope_of(&left.plan);
        let mut right_outer = outer.to_vec();
        right_outer.push(left_scope.clone());
        // A positive LIMIT cannot change the truth value of EXISTS: the
        // predicate only asks whether at least one row survives. Remove a
        // positive top-level limit before binding so correlated EXISTS can
        // still be decorrelated into a keyed semi/anti join. LIMIT 0 becomes
        // an empty relation (always false for EXISTS, always true for NOT
        // EXISTS), OFFSET 0 is removed as a no-op. A keyed correlated IN
        // also only asks whether the correlated key has a matching row, so
        // a positive top-level LIMIT is a no-op there; uncorrelated IN keeps
        // its paging because LIMIT changes the set being tested.
        let (right, positive_offset) = if matches!(right_key, Expr::Literal(_)) {
            let right = strip_exists_noop_paging(right);
            peel_positive_exists_offset(right)
        } else if matches!(&right_key, Expr::Column(name) if name == "*") {
            (strip_correlated_in_paging(right), None)
        } else {
            (right, None)
        };
        let right = self.bind(right, &right_outer)?;
        let (lateral, deeper): (Vec<_>, Vec<_>) = right
            .correlations
            .into_iter()
            .partition(|correlation| correlation.depth() == outer.len());
        if !deeper.is_empty() {
            return Err(KaveonError::Sql(
                "a correlation reaching past an IN or EXISTS subquery is not supported".into(),
            ));
        }
        if let Some(offset) = positive_offset {
            let keys = lateral
                .iter()
                .map(|correlation| match correlation {
                    Correlation::Key { inner, outer, .. } => Ok((inner.clone(), outer.clone())),
                    Correlation::Residual { .. } => Err(KaveonError::Sql(
                        "a positive correlated OFFSET requires equality correlations only".into(),
                    )),
                })
                .collect::<Result<Vec<_>>>()?;
            let Some((inner, _)) = keys.first() else {
                return Err(KaveonError::Sql(
                    "a positive correlated OFFSET requires at least one equality correlation"
                        .into(),
                ));
            };
            let left_key = self.bind_expr(left_key, &left_scope, None)?;
            let count = AggregateExpr::Count {
                expr: Expr::Star,
                distinct: false,
            };
            let aggregate = LogicalPlan::Aggregate {
                input: Box::new(right.plan),
                group_by: keys
                    .iter()
                    .map(|(inner, _)| Expr::Column(inner.clone()))
                    .collect(),
                aggregates: vec![count],
            };
            let threshold = Expr::BinaryOp {
                // Keep the aggregate call here. The physical planner binds
                // it to the correct fragment or node-local output name for
                // the stage that consumes this filter.
                left: Box::new(Expr::Function {
                    name: "COUNT".into(),
                    args: vec![Expr::Star],
                }),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal(kaveon_core::ScalarValue::Int64(
                    i64::try_from(offset)
                        .map_err(|_| KaveonError::Sql("correlated OFFSET is too large".into()))?,
                ))),
            };
            let predicate = if anti {
                Expr::And(
                    Box::new(threshold),
                    Box::new(Expr::IsNotNull(Box::new(Expr::Column(inner.clone())))),
                )
            } else {
                threshold
            };
            let mut projected = vec![Expr::Column(inner.clone())];
            let residual = conjoin(
                keys.iter()
                    .skip(1)
                    .map(|(inner, outer)| {
                        let name = self.fresh_name();
                        projected.push(Expr::Alias {
                            expr: Box::new(Expr::Column(inner.clone())),
                            name: name.clone(),
                        });
                        Expr::BinaryOp {
                            left: Box::new(Expr::Column(name)),
                            op: BinaryOp::Eq,
                            right: Box::new(Expr::Column(outer.clone())),
                        }
                    })
                    .collect(),
            )
            .map(|predicate| self.bind_expr(predicate, &left_scope, None))
            .transpose()?;
            let filtered = LogicalPlan::Filter {
                input: Box::new(aggregate),
                predicate,
            };
            let right = if projected.len() == 1 {
                filtered
            } else {
                LogicalPlan::Project {
                    input: Box::new(filtered),
                    columns: projected,
                }
            };
            return Ok(BoundSemiJoin {
                left: Box::new(left.plan),
                right: Box::new(right),
                left_key,
                right_key: Expr::Column(inner.clone()),
                residual,
                correlations: left.correlations,
            });
        }
        let correlated_key = lateral.iter().find_map(|correlation| match correlation {
            Correlation::Key { inner, .. } => Some(inner.as_str()),
            Correlation::Residual { .. } => None,
        });
        let correlated_projection_matches = correlated_key.is_some()
            && lateral
                .iter()
                .filter(|correlation| matches!(correlation, Correlation::Key { .. }))
                .count()
                == 1
            && correlated_projection_matches(&right.plan, correlated_key);
        if lateral.is_empty() {
            let left_key = self.bind_expr(left_key, &left_scope, None)?;
            // The subquery side binds its key by position (`*`) or not at
            // all (a literal marks an uncorrelated EXISTS).
            let right_key = match right_key {
                Expr::Column(name) if name != "*" => {
                    self.bind_expr(Expr::Column(name), &self.scope_of(&right.plan), None)?
                }
                other => other,
            };
            return Ok(BoundSemiJoin {
                left: Box::new(left.plan),
                right: Box::new(right.plan),
                left_key,
                right_key,
                residual,
                correlations: left.correlations,
            });
        }
        // A correlated IN can be reduced to a keyed semi/anti join when its
        // subquery projects the equality correlation key first. Residual
        // predicates ride through the same residual-aware operator as EXISTS;
        // unlike NOT EXISTS, NOT IN must retain right-side NULLs so the
        // operator can apply SQL's set-wide NULL rule.
        if matches!(&right_key, Expr::Column(name) if name == "*")
            && residual.is_none()
            && lateral.len() == 1
            && let [Correlation::Key { inner, .. }] = lateral.as_slice()
            && correlated_projection_matches
        {
            let left_key = self.bind_expr(left_key, &left_scope, None)?;
            let right_key = self.bind_expr(
                Expr::Column(inner.clone()),
                &self.scope_of(&right.plan),
                None,
            )?;
            return Ok(BoundSemiJoin {
                left: Box::new(left.plan),
                right: Box::new(right.plan),
                left_key,
                right_key,
                residual: None,
                correlations: left.correlations,
            });
        }
        let correlated_in = matches!(&right_key, Expr::Column(name) if name == "*")
            && correlated_projection_matches;
        if !matches!(right_key, Expr::Literal(_)) && !correlated_in {
            return Err(KaveonError::Sql(
                "a correlated IN subquery is not supported".into(),
            ));
        }
        if let Some(residual) = residual {
            return Err(KaveonError::Sql(format!(
                "a correlated EXISTS cannot carry a residual before binding: {residual:?}"
            )));
        }
        let mut keys = Vec::new();
        let mut residuals = Vec::new();
        for correlation in lateral {
            match correlation {
                Correlation::Key { inner, outer, .. } => keys.push((inner, outer)),
                Correlation::Residual {
                    predicate, inner, ..
                } => residuals.push((predicate, inner)),
            }
        }
        let mut keys = keys.into_iter();
        let Some((key_inner, key_outer)) = keys.next() else {
            return Err(KaveonError::Sql(format!(
                "EXISTS correlated without an equality between a column of the subquery and a column of the enclosing query is not supported: {:?}",
                residuals[0].0
            )));
        };
        let key = Expr::Column(key_inner);
        // The subquery's columns a residual reads, projected beside the
        // key under names of their own; the residual reads them by those
        // names, and the enclosing query's columns as the left names them.
        let mut columns = vec![key.clone()];
        let mut projected: Vec<(String, String)> = Vec::new();
        let mut project = |inner: String| -> String {
            if let Some((_, name)) = projected.iter().find(|(column, _)| *column == inner) {
                return name.clone();
            }
            let name = self.fresh_name();
            columns.push(Expr::Alias {
                expr: Box::new(Expr::Column(inner.clone())),
                name: name.clone(),
            });
            projected.push((inner, name.clone()));
            name
        };
        let mut conjuncts = Vec::new();
        for (inner, outer) in keys {
            conjuncts.push(Expr::BinaryOp {
                left: Box::new(Expr::Column(project(inner))),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Column(outer)),
            });
        }
        for (mut predicate, inner) in residuals {
            for column in inner {
                let name = project(column.clone());
                predicate = rename_column(predicate, &column, &name);
            }
            conjuncts.push(predicate);
        }
        let residual = conjoin(conjuncts)
            .map(|predicate| self.bind_expr(predicate, &left_scope, None))
            .transpose()?;
        let mut subquery = right.plan;
        if anti && !correlated_in {
            subquery = LogicalPlan::Filter {
                input: Box::new(subquery),
                predicate: Expr::IsNotNull(Box::new(key.clone())),
            };
        }
        // Alone, the key binds by position; beside residual columns, by
        // name.
        let right_key = if columns.len() == 1 {
            Expr::Column("*".into())
        } else {
            key
        };
        let subquery = LogicalPlan::Project {
            input: Box::new(subquery),
            columns,
        };
        let left_key = Expr::Column(self.bind_name(&key_outer, &left_scope)?);
        Ok(BoundSemiJoin {
            left: Box::new(left.plan),
            right: Box::new(subquery),
            left_key,
            right_key,
            residual,
            correlations: left.correlations,
        })
    }

    /// Route the conjuncts of a filter into the join tree below it. What
    /// comes back stays above `plan`.
    fn route(&self, conjuncts: Vec<Expr>, plan: LogicalPlan) -> Result<(LogicalPlan, Vec<Expr>)> {
        match plan {
            LogicalPlan::Join {
                left,
                right,
                join_type: JoinType::Cross | JoinType::Inner,
                condition,
                using_columns,
                distribution,
            } => {
                let left_scope = self.scope_of(&left).qualified_by(relation_qualifier(&left));
                let right_scope = self
                    .scope_of(&right)
                    .qualified_by(relation_qualifier(&right));
                let mut left_parts = Vec::new();
                let mut right_parts = Vec::new();
                let mut keys = Vec::new();
                let mut above = Vec::new();
                for conjunct in conjuncts {
                    match placement(&conjunct, &left_scope, &right_scope) {
                        Placement::Left => left_parts.push(conjunct),
                        Placement::Right => right_parts.push(conjunct),
                        Placement::Key(left, right) => keys.push((left, right)),
                        Placement::Above => above.push(conjunct),
                    }
                }
                let left = self.place(left_parts, *left)?;
                let right = self.place(right_parts, *right)?;
                let mut condition = condition;
                for (left_key, right_key) in keys {
                    let key = Expr::BinaryOp {
                        left: Box::new(Expr::Column(left_key)),
                        op: BinaryOp::Eq,
                        right: Box::new(Expr::Column(right_key)),
                    };
                    condition = Some(match condition {
                        Some(condition) => Expr::And(Box::new(condition), Box::new(key)),
                        None => key,
                    });
                }
                let join_type = if condition.is_some() {
                    JoinType::Inner
                } else {
                    JoinType::Cross
                };
                Ok((
                    LogicalPlan::Join {
                        left: Box::new(left),
                        right: Box::new(right),
                        join_type,
                        condition,
                        using_columns,
                        distribution,
                    },
                    above,
                ))
            }
            // A semi or anti join emits rows of its left input, so a filter
            // on its output is a filter on that input.
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            } => Ok((
                LogicalPlan::SemiJoin {
                    left: Box::new(self.place(conjuncts, *left)?),
                    right,
                    left_key,
                    right_key,
                    residual,
                },
                Vec::new(),
            )),
            LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            } => Ok((
                LogicalPlan::AntiJoin {
                    left: Box::new(self.place(conjuncts, *left)?),
                    right,
                    left_key,
                    right_key,
                    residual,
                },
                Vec::new(),
            )),
            other => Ok((other, conjuncts)),
        }
    }

    /// Route `conjuncts` into `plan`, filtering above it what does not go
    /// further down.
    fn place(&self, conjuncts: Vec<Expr>, plan: LogicalPlan) -> Result<LogicalPlan> {
        if conjuncts.is_empty() {
            return Ok(plan);
        }
        let (plan, residual) = self.route(conjuncts, plan)?;
        let scope = self.scope_of(&plan);
        self.filtered(plan, residual, &scope, None)
    }

    fn filtered(
        &self,
        plan: LogicalPlan,
        conjuncts: Vec<Expr>,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<LogicalPlan> {
        let bound = conjuncts
            .into_iter()
            .map(|conjunct| self.bind_expr(conjunct, scope, aggregate_input))
            .collect::<Result<Vec<_>>>()?;
        Ok(match conjoin(bound) {
            Some(predicate) => LogicalPlan::Filter {
                input: Box::new(plan),
                predicate,
            },
            None => plan,
        })
    }

    /// A projected column keeps the name it was written with.
    fn bind_projected(
        &self,
        expr: Expr,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<Expr> {
        match expr {
            Expr::Column(name) if name != "*" => {
                let bound = self.bind_name(&name, scope)?;
                Ok(if bound == name {
                    Expr::Column(name)
                } else {
                    Expr::Alias {
                        expr: Box::new(Expr::Column(bound)),
                        name,
                    }
                })
            }
            other => self.bind_expr(other, scope, aggregate_input),
        }
    }

    fn bind_aggregate(&self, aggregate: AggregateExpr, scope: &Scope) -> Result<AggregateExpr> {
        Ok(match aggregate {
            AggregateExpr::Count { expr, distinct } => AggregateExpr::Count {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Sum { expr, distinct } => AggregateExpr::Sum {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Avg { expr, distinct } => AggregateExpr::Avg {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Min(expr) => AggregateExpr::Min(self.bind_expr(expr, scope, None)?),
            AggregateExpr::Max(expr) => AggregateExpr::Max(self.bind_expr(expr, scope, None)?),
            AggregateExpr::ApproxDistinct { expr, as_count } => AggregateExpr::ApproxDistinct {
                expr: self.bind_expr(expr, scope, None)?,
                as_count,
            },
            AggregateExpr::ApproxPercentile { expr, percentiles } => {
                AggregateExpr::ApproxPercentile {
                    expr: self.bind_expr(expr, scope, None)?,
                    percentiles,
                }
            }
            AggregateExpr::ApproxDistinctState(expr) => {
                AggregateExpr::ApproxDistinctState(self.bind_expr(expr, scope, None)?)
            }
            AggregateExpr::ColumnStatistics(expr) => {
                AggregateExpr::ColumnStatistics(self.bind_expr(expr, scope, None)?)
            }
        })
    }

    fn bind_name(&self, name: &str, scope: &Scope) -> Result<String> {
        if name == "*" {
            return Ok(name.to_owned());
        }
        match scope.resolve(name) {
            Resolution::Unique(physical) if scope.joined => Ok(physical),
            Resolution::Unique(_) | Resolution::Unresolved => Ok(name.to_owned()),
            Resolution::Ambiguous => Err(KaveonError::Sql(format!("column '{name}' is ambiguous"))),
        }
    }

    fn bind_expr(
        &self,
        expr: Expr,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<Expr> {
        let bind = |expr: Box<Expr>| -> Result<Box<Expr>> {
            self.bind_expr(*expr, scope, aggregate_input).map(Box::new)
        };
        Ok(match expr {
            Expr::Column(name) => Expr::Column(self.bind_name(&name, scope)?),
            Expr::Literal(_) | Expr::Star => expr,
            Expr::Alias { expr, name } => Expr::Alias {
                expr: bind(expr)?,
                name,
            },
            Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
                left: bind(left)?,
                op,
                right: bind(right)?,
            },
            Expr::And(left, right) => Expr::And(bind(left)?, bind(right)?),
            Expr::Or(left, right) => Expr::Or(bind(left)?, bind(right)?),
            Expr::Not(inner) => Expr::Not(bind(inner)?),
            Expr::IsNull(inner) => Expr::IsNull(bind(inner)?),
            Expr::IsNotNull(inner) => Expr::IsNotNull(bind(inner)?),
            Expr::Cast { expr, data_type } => Expr::Cast {
                expr: bind(expr)?,
                data_type,
            },
            Expr::Extract { field, expr } => Expr::Extract {
                field,
                expr: bind(expr)?,
            },
            // An aggregate reference above the aggregate binds its
            // argument the way the aggregate did.
            Expr::Function { name, args } => {
                let scope = match aggregate_input {
                    Some(input) if is_aggregate(&name) => input,
                    _ => scope,
                };
                Expr::Function {
                    name,
                    args: args
                        .into_iter()
                        .map(|arg| self.bind_expr(arg, scope, None))
                        .collect::<Result<_>>()?,
                }
            }
            Expr::WindowFunction {
                name,
                args,
                partition_by,
                order_by,
                frame,
            } => Expr::WindowFunction {
                name,
                args: args
                    .into_iter()
                    .map(|arg| self.bind_expr(arg, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                partition_by: partition_by
                    .into_iter()
                    .map(|expr| self.bind_expr(expr, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                order_by: order_by
                    .into_iter()
                    .map(|(expr, ascending, nulls_first)| {
                        self.bind_expr(expr, scope, aggregate_input)
                            .map(|expr| (expr, ascending, nulls_first))
                    })
                    .collect::<Result<_>>()?,
                frame,
            },
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => Expr::Case {
                operand: operand.map(bind).transpose()?,
                when_then: when_then
                    .into_iter()
                    .map(|(when, then)| {
                        Ok((
                            self.bind_expr(when, scope, aggregate_input)?,
                            self.bind_expr(then, scope, aggregate_input)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
                else_expr: else_expr.map(bind).transpose()?,
            },
            Expr::Like {
                expr,
                pattern,
                negated,
                case_insensitive,
            } => Expr::Like {
                expr: bind(expr)?,
                pattern: bind(pattern)?,
                negated,
                case_insensitive,
            },
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => Expr::Between {
                expr: bind(expr)?,
                low: bind(low)?,
                high: bind(high)?,
                negated,
            },
            Expr::InList {
                expr,
                list,
                negated,
            } => Expr::InList {
                expr: bind(expr)?,
                list: list
                    .into_iter()
                    .map(|item| self.bind_expr(item, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                negated,
            },
        })
    }

    fn join_scope(&self, left: &LogicalPlan, right: &LogicalPlan) -> Scope {
        let mut columns = self
            .scope_of(left)
            .qualified_by(relation_qualifier(left))
            .columns;
        columns.extend(
            self.scope_of(right)
                .qualified_by(relation_qualifier(right))
                .columns,
        );
        Scope {
            columns,
            joined: true,
        }
    }

    /// The output columns of a bound plan.
    fn scope_of(&self, plan: &LogicalPlan) -> Scope {
        match plan {
            LogicalPlan::Scan {
                table,
                alias,
                columns,
            } => {
                let qualifier = alias
                    .clone()
                    .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned());
                let Ok(resolved) = self.catalog.resolve_table(&TableReference::parse(table)) else {
                    return Scope::default();
                };
                Scope {
                    columns: resolved
                        .table
                        .arrow_schema
                        .fields()
                        .iter()
                        .filter(|field| {
                            columns
                                .as_ref()
                                .is_none_or(|columns| columns.contains(field.name()))
                        })
                        .map(|field| Column {
                            qualifier: Some(qualifier.clone()),
                            name: field.name().clone(),
                            physical: field.name().clone(),
                        })
                        .collect(),
                    joined: false,
                }
            }
            LogicalPlan::Unnest {
                columns,
                with_ordinality,
                ordinality_column,
                ..
            } => Scope {
                columns: columns
                    .iter()
                    .map(|column| Column {
                        qualifier: None,
                        name: column.clone(),
                        physical: column.clone(),
                    })
                    .chain((*with_ordinality).then(|| Column {
                        qualifier: None,
                        name: ordinality_column.clone(),
                        physical: ordinality_column.clone(),
                    }))
                    .collect(),
                joined: false,
            },
            LogicalPlan::Join { left, right, .. } => self.join_scope(left, right),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => self.scope_of(input),
            LogicalPlan::SemiJoin { left, .. }
            | LogicalPlan::AntiJoin { left, .. }
            | LogicalPlan::Intersect { left, .. }
            | LogicalPlan::Except { left, .. } => self.scope_of(left),
            LogicalPlan::Union { inputs, .. } => Scope {
                columns: inputs
                    .first()
                    .map(|input| self.scope_of(input).columns)
                    .unwrap_or_default(),
                joined: false,
            },
            LogicalPlan::Project { input, columns } => {
                let input = self.scope_of(input);
                let mut out = Vec::new();
                for column in columns {
                    match column {
                        Expr::Star => out.extend(input.columns.iter().cloned()),
                        Expr::Alias { name, .. } => {
                            let (qualifier, bare) = match name.rsplit_once('.') {
                                Some((qualifier, bare)) => {
                                    (Some(qualifier.to_owned()), bare.to_owned())
                                }
                                None => (None, name.clone()),
                            };
                            out.push(Column {
                                qualifier,
                                name: bare,
                                physical: name.clone(),
                            });
                        }
                        // The output field carries the written name.
                        Expr::Column(name) => {
                            let (qualifier, bare) = match name.rsplit_once('.') {
                                Some((qualifier, bare)) => (Some(qualifier.to_owned()), bare),
                                None => (None, name.as_str()),
                            };
                            out.push(Column {
                                qualifier,
                                name: bare.to_owned(),
                                physical: name.clone(),
                            });
                        }
                        // Named by its printed form; not addressable by name.
                        _ => {}
                    }
                }
                Scope {
                    columns: out,
                    joined: false,
                }
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.scope_of(input);
                let mut out = Vec::new();
                for key in group_by {
                    if let Expr::Column(name) = key {
                        out.push(
                            input
                                .columns
                                .iter()
                                .find(|column| &column.physical == name)
                                .cloned()
                                .unwrap_or_else(|| Column {
                                    qualifier: None,
                                    name: name.clone(),
                                    physical: name.clone(),
                                }),
                        );
                    }
                }
                for aggregate in aggregates {
                    let name = aggregate_output_name(aggregate);
                    out.push(Column {
                        qualifier: None,
                        name: name.clone(),
                        physical: name,
                    });
                }
                Scope {
                    columns: out,
                    joined: false,
                }
            }
        }
    }
}

/// Where a filter conjunct over a join belongs, by the sides its columns
/// resolve on.
fn placement(conjunct: &Expr, left: &Scope, right: &Scope) -> Placement {
    let mut references = Vec::new();
    column_references(conjunct, &mut references);
    if references.is_empty() {
        return Placement::Above;
    }
    let on_left = references
        .iter()
        .all(|reference| left.holds(reference) && !right.holds(reference));
    let on_right = references
        .iter()
        .all(|reference| right.holds(reference) && !left.holds(reference));
    if on_left {
        return Placement::Left;
    }
    if on_right {
        return Placement::Right;
    }
    if let Expr::BinaryOp {
        left: a,
        op: BinaryOp::Eq,
        right: b,
    } = conjunct
        && let (Expr::Column(a), Expr::Column(b)) = (a.as_ref(), b.as_ref())
    {
        let side = |reference: &str| match (left.resolve(reference), right.resolve(reference)) {
            (Resolution::Unique(physical), Resolution::Unresolved) => Some((true, physical)),
            (Resolution::Unresolved, Resolution::Unique(physical)) => Some((false, physical)),
            _ => None,
        };
        if let (Some((a_left, a_physical)), Some((b_left, b_physical))) = (side(a), side(b)) {
            if a_left && !b_left {
                return Placement::Key(a_physical, b_physical);
            }
            if !a_left && b_left {
                return Placement::Key(b_physical, a_physical);
            }
        }
    }
    Placement::Above
}

/// Whether `expr` applies an aggregate function anywhere.
fn contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function { name, args } => is_aggregate(name) || args.iter().any(contains_aggregate),
        Expr::Column(_) | Expr::Literal(_) | Expr::Star | Expr::WindowFunction { .. } => false,
        Expr::Alias { expr, .. }
        | Expr::Not(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::Cast { expr, .. }
        | Expr::Extract { expr, .. } => contains_aggregate(expr),
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            contains_aggregate(left) || contains_aggregate(right)
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            operand
                .iter()
                .chain(else_expr)
                .any(|expr| contains_aggregate(expr))
                || when_then
                    .iter()
                    .any(|(when, then)| contains_aggregate(when) || contains_aggregate(then))
        }
        Expr::Like { expr, pattern, .. } => contains_aggregate(expr) || contains_aggregate(pattern),
        Expr::Between {
            expr, low, high, ..
        } => contains_aggregate(expr) || contains_aggregate(low) || contains_aggregate(high),
        Expr::InList { expr, list, .. } => {
            contains_aggregate(expr) || list.iter().any(contains_aggregate)
        }
    }
}

/// `expr` with every reference to the column `from` reading `to`.
fn rename_column(expr: Expr, from: &str, to: &str) -> Expr {
    let rename = |expr: Box<Expr>| Box::new(rename_column(*expr, from, to));
    match expr {
        Expr::Column(name) if name == from => Expr::Column(to.to_owned()),
        Expr::Column(_) | Expr::Literal(_) | Expr::Star => expr,
        Expr::Alias { expr, name } => Expr::Alias {
            expr: rename(expr),
            name,
        },
        Expr::Not(expr) => Expr::Not(rename(expr)),
        Expr::IsNull(expr) => Expr::IsNull(rename(expr)),
        Expr::IsNotNull(expr) => Expr::IsNotNull(rename(expr)),
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: rename(expr),
            data_type,
        },
        Expr::Extract { field, expr } => Expr::Extract {
            field,
            expr: rename(expr),
        },
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: rename(left),
            op,
            right: rename(right),
        },
        Expr::And(left, right) => Expr::And(rename(left), rename(right)),
        Expr::Or(left, right) => Expr::Or(rename(left), rename(right)),
        Expr::Function { name, args } => Expr::Function {
            name,
            args: args
                .into_iter()
                .map(|arg| rename_column(arg, from, to))
                .collect(),
        },
        Expr::WindowFunction {
            name,
            args,
            partition_by,
            order_by,
            frame,
        } => Expr::WindowFunction {
            name,
            args: args
                .into_iter()
                .map(|arg| rename_column(arg, from, to))
                .collect(),
            partition_by: partition_by
                .into_iter()
                .map(|expr| rename_column(expr, from, to))
                .collect(),
            order_by: order_by
                .into_iter()
                .map(|(expr, ascending, nulls_first)| {
                    (rename_column(expr, from, to), ascending, nulls_first)
                })
                .collect(),
            frame,
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => Expr::Case {
            operand: operand.map(rename),
            when_then: when_then
                .into_iter()
                .map(|(when, then)| (rename_column(when, from, to), rename_column(then, from, to)))
                .collect(),
            else_expr: else_expr.map(rename),
        },
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: rename(expr),
            pattern: rename(pattern),
            negated,
            case_insensitive,
        },
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: rename(expr),
            low: rename(low),
            high: rename(high),
            negated,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => Expr::InList {
            expr: rename(expr),
            list: list
                .into_iter()
                .map(|item| rename_column(item, from, to))
                .collect(),
            negated,
        },
    }
}

/// The column references of `expr` outside aggregate function arguments.
fn free_column_references(expr: &Expr, into: &mut Vec<String>) {
    match expr {
        Expr::Function { name, .. } if is_aggregate(name) => {}
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            free_column_references(left, into);
            free_column_references(right, into);
        }
        Expr::Not(inner) | Expr::Alias { expr: inner, .. } => free_column_references(inner, into),
        other => column_references(other, into),
    }
}

fn column_references(expr: &Expr, into: &mut Vec<String>) {
    match expr {
        Expr::Column(name) => {
            if name != "*" {
                into.push(name.clone());
            }
        }
        Expr::Literal(_) | Expr::Star => {}
        Expr::Alias { expr, .. }
        | Expr::Not(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::Cast { expr, .. }
        | Expr::Extract { expr, .. } => column_references(expr, into),
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            column_references(left, into);
            column_references(right, into);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                column_references(arg, into);
            }
        }
        Expr::WindowFunction {
            args,
            partition_by,
            order_by,
            ..
        } => {
            for expr in args
                .iter()
                .chain(partition_by)
                .chain(order_by.iter().map(|(expr, _, _)| expr))
            {
                column_references(expr, into);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            for expr in operand.iter().chain(else_expr) {
                column_references(expr, into);
            }
            for (when, then) in when_then {
                column_references(when, into);
                column_references(then, into);
            }
        }
        Expr::Like { expr, pattern, .. } => {
            column_references(expr, into);
            column_references(pattern, into);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            column_references(expr, into);
            column_references(low, into);
            column_references(high, into);
        }
        Expr::InList { expr, list, .. } => {
            column_references(expr, into);
            for item in list {
                column_references(item, into);
            }
        }
    }
}

/// The relation a join qualifies an input's columns with: a scan (or a
/// filtered scan) under its alias or table name; anything else keeps its
/// own output names.
fn relation_qualifier(plan: &LogicalPlan) -> Option<&str> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .as_deref()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table)),
        ),
        LogicalPlan::Filter { input, .. } => relation_qualifier(input),
        LogicalPlan::Project { input, columns }
            if columns.len() == 1 && matches!(columns.first(), Some(Expr::Star)) =>
        {
            relation_qualifier(input)
        }
        _ => None,
    }
}

fn uncorrelated(bound: &Bound, clause: &str) -> Result<()> {
    if bound.correlations.is_empty() {
        Ok(())
    } else {
        Err(KaveonError::Sql(format!(
            "{clause} inside a correlated subquery is not supported"
        )))
    }
}

/// Peel the single bounded paging chain used by a scalar subquery while
/// retaining its projection and sort. The returned plan has no LIMIT/OFFSET
/// or Sort nodes; the caller applies the ordering through a per-key window.
/// Refusing duplicate paging/order nodes keeps this rewrite from changing a
/// nested derived relation's semantics.
type ScalarPagingPlan = (LogicalPlan, usize, usize, Vec<(Expr, bool, bool)>);

fn peel_scalar_paging(plan: LogicalPlan) -> Option<ScalarPagingPlan> {
    fn peel(
        plan: LogicalPlan,
        offset: &mut Option<usize>,
        limit: &mut Option<usize>,
        order: &mut Option<Vec<(Expr, bool, bool)>>,
    ) -> Option<LogicalPlan> {
        match plan {
            LogicalPlan::Limit { input, count } => {
                if limit.replace(count).is_some() {
                    return None;
                }
                Some(peel(*input, offset, limit, order)?)
            }
            LogicalPlan::Offset { input, count } => {
                if offset.replace(count).is_some() {
                    return None;
                }
                Some(peel(*input, offset, limit, order)?)
            }
            LogicalPlan::Sort { input, order_by } => {
                if order.replace(order_by).is_some() {
                    return None;
                }
                Some(peel(*input, offset, limit, order)?)
            }
            LogicalPlan::Project { input, columns } => Some(LogicalPlan::Project {
                input: Box::new(peel(*input, offset, limit, order)?),
                columns,
            }),
            other => Some(other),
        }
    }

    let mut offset = None;
    let mut limit = None;
    let mut order = None;
    let plan = peel(plan, &mut offset, &mut limit, &mut order)?;
    Some((plan, offset?, limit?, order.unwrap_or_default()))
}

fn contains_paging(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Limit { .. } | LogicalPlan::Offset { .. } => true,
        _ => plan.inputs().into_iter().any(contains_paging),
    }
}

fn strip_exists_noop_paging(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Limit { input, count } if count > 0 => *input,
        LogicalPlan::Limit { count: 0, .. } => LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Unnest {
                expressions: vec![Expr::Function {
                    name: "ARRAY".to_owned(),
                    args: vec![Expr::Literal(kaveon_core::ScalarValue::Int64(0))],
                }],
                columns: vec!["_kaveon_empty".to_owned()],
                with_ordinality: false,
                ordinality_column: "ordinality".to_owned(),
                ordinality_start: 1,
            }),
            predicate: Expr::Literal(kaveon_core::ScalarValue::Bool(false)),
        },
        // OFFSET 0 is also a semantic no-op for EXISTS.  Do not remove a
        // positive offset: unlike LIMIT, it can change whether any row
        // remains and therefore cannot be represented by a semi/anti join.
        LogicalPlan::Offset { input, count: 0 } => *input,
        plan => plan,
    }
}

/// A keyed correlated IN is existential with respect to the enclosing row:
/// once its projected equality key matches, retaining any one row is enough
/// to determine membership. A positive top-level LIMIT therefore cannot
/// change the result. LIMIT 0 is lowered to an explicit empty relation;
/// OFFSET 0 is a no-op, while a positive OFFSET remains because it changes
/// the tested set.
fn strip_correlated_in_paging(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input: Box::new(strip_correlated_in_paging(*input)),
            columns,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(strip_correlated_in_paging(*input)),
        },
        LogicalPlan::Limit { input, count } if count > 0 => strip_correlated_in_paging(*input),
        LogicalPlan::Limit { count: 0, .. } => LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Unnest {
                expressions: vec![Expr::Function {
                    name: "ARRAY".to_owned(),
                    args: vec![Expr::Literal(kaveon_core::ScalarValue::Int64(0))],
                }],
                columns: vec!["_kaveon_empty".to_owned()],
                with_ordinality: false,
                ordinality_column: "ordinality".to_owned(),
                ordinality_start: 1,
            }),
            predicate: Expr::Literal(kaveon_core::ScalarValue::Bool(false)),
        },
        LogicalPlan::Offset { input, count: 0 } => *input,
        LogicalPlan::Sort { input, .. } => *input,
        plan => plan,
    }
}

fn peel_positive_exists_offset(plan: LogicalPlan) -> (LogicalPlan, Option<usize>) {
    match plan {
        LogicalPlan::Offset { input, count } if count > 0 => (*input, Some(count)),
        plan => (plan, None),
    }
}

/// Whether a correlated `IN` subquery exposes its equality key as the first
/// projected value. `DISTINCT` is transparent here: it changes multiplicity,
/// which a semi/anti join already ignores, but it must not make an otherwise
/// keyed subquery fall through to the unsupported-correlated diagnostic.
fn correlated_projection_matches(plan: &LogicalPlan, inner: Option<&str>) -> bool {
    let Some(inner) = inner else {
        return false;
    };
    let plan = match plan {
        LogicalPlan::Distinct { input } => input.as_ref(),
        other => other,
    };
    let LogicalPlan::Project { input, columns } = plan else {
        return false;
    };
    let first = columns.first().and_then(|column| match column {
        Expr::Column(name) => Some(name.as_str()),
        _ => None,
    });
    let source = columns.iter().find_map(|column| match column {
        Expr::Alias { expr, name } if name == inner => match expr.as_ref() {
            Expr::Column(source) => Some(source.as_str()),
            _ => None,
        },
        _ => None,
    });
    if first
        .zip(source)
        .is_some_and(|(first, source)| first.rsplit('.').next() == source.rsplit('.').next())
    {
        return true;
    }
    if let (Some(first), Some(source)) = (first, source)
        && projection_alias_resolves_to_first(input, source, first)
    {
        return true;
    }
    matches!(columns.first(), Some(Expr::Column(_)))
        && correlated_projection_matches(input, Some(inner))
}

fn projection_alias_resolves_to_first(plan: &LogicalPlan, alias: &str, first: &str) -> bool {
    match plan {
        LogicalPlan::Distinct { input } => projection_alias_resolves_to_first(input, alias, first),
        LogicalPlan::Project { input, columns } => {
            for column in columns {
                if let Expr::Alias { expr, name } = column
                    && name == alias
                    && let Expr::Column(source) = expr.as_ref()
                {
                    if source.rsplit('.').next() == first.rsplit('.').next() {
                        return true;
                    }
                    return projection_alias_resolves_to_first(input, source, first);
                }
            }
            projection_alias_resolves_to_first(input, alias, first)
        }
        _ => false,
    }
}

fn is_aggregate(name: &str) -> bool {
    kaveon_core::is_aggregate_function(name)
}

fn contains_count_aggregate(plan: &LogicalPlan) -> bool {
    if let LogicalPlan::Aggregate { aggregates, .. } = plan
        && aggregates
            .iter()
            .any(|aggregate| matches!(aggregate, AggregateExpr::Count { .. }))
    {
        return true;
    }
    plan.inputs().into_iter().any(contains_count_aggregate)
}

fn coalesce_count_projection(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input,
            columns: columns
                .into_iter()
                .map(|column| match column {
                    Expr::Alias { expr, name } if name.starts_with("__kaveon_scalar_") => {
                        let Expr::Function {
                            name: function,
                            args,
                        } = *expr
                        else {
                            return Expr::Alias { expr, name };
                        };
                        if !function.eq_ignore_ascii_case("COUNT") || args.len() != 1 {
                            return Expr::Alias {
                                expr: Box::new(Expr::Function {
                                    name: function,
                                    args,
                                }),
                                name,
                            };
                        }
                        Expr::Alias {
                            expr: Box::new(Expr::Function {
                                name: "COALESCE".into(),
                                args: vec![
                                    Expr::Function {
                                        name: function,
                                        args,
                                    },
                                    Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(0)),
                                ],
                            }),
                            name,
                        }
                    }
                    other => other,
                })
                .collect(),
        },
        other => other,
    }
}

/// The aggregate's output column as the local planner names it.
fn aggregate_output_name(aggregate: &AggregateExpr) -> String {
    aggregate.output_name()
}

fn conjuncts(expr: Expr) -> Vec<Expr> {
    fn split(expr: Expr, into: &mut Vec<Expr>) {
        match expr {
            Expr::And(left, right) => {
                split(*left, into);
                split(*right, into);
            }
            other => into.push(other),
        }
    }
    let mut out = Vec::new();
    split(expr, &mut out);
    out
}

fn conjoin(mut conjuncts: Vec<Expr>) -> Option<Expr> {
    let mut result = conjuncts.pop()?;
    while let Some(conjunct) = conjuncts.pop() {
        result = Expr::And(Box::new(conjunct), Box::new(result));
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use kaveon_core::predicate::ScalarValue;
    use kaveon_core::{
        AccessPattern, CatalogProvider, DataFormat, MemoryCatalog, StorageType, TableMeta,
    };
    use kaveon_sql::logical_plan::{sql_to_logical_plan, sql_to_logical_plan_for_binder};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn catalog() -> CatalogManager {
        let mut memory = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: PathBuf::from("."),
            },
        )
        .with_schema("tpch");
        let tables: [(&str, &[&str]); 4] = [
            (
                "customer",
                &["c_custkey", "c_name", "c_nationkey", "c_mktsegment"],
            ),
            ("orders", &["o_orderkey", "o_custkey", "o_orderdate"]),
            ("lineitem", &["l_orderkey", "l_quantity", "l_shipdate"]),
            ("nation", &["n_nationkey", "n_name", "n_regionkey"]),
        ];
        for (table, columns) in tables {
            let fields = columns
                .iter()
                .map(|column| Field::new(*column, DataType::Int64, true))
                .collect::<Vec<_>>();
            memory
                .register_table(
                    "tpch",
                    TableMeta {
                        name: table.to_owned(),
                        arrow_schema: Arc::new(Schema::new(fields)),
                        location: format!("{table}.parquet"),
                        access: AccessPattern::Shortcut,
                        format: DataFormat::Parquet,
                    },
                )
                .unwrap();
        }
        let mut manager = CatalogManager::new("lake", "tpch");
        manager.register_catalog(Box::new(memory));
        manager
    }

    fn bound(sql: &str) -> LogicalPlan {
        let mut plan = sql_to_logical_plan_for_binder(sql).unwrap();
        qualify(&mut plan);
        bind(plan, &catalog()).unwrap()
    }

    fn qualify(plan: &mut LogicalPlan) {
        match plan {
            LogicalPlan::Scan { table, .. } => *table = format!("lake.tpch.{table}"),
            LogicalPlan::Join { left, right, .. }
            | LogicalPlan::SemiJoin { left, right, .. }
            | LogicalPlan::AntiJoin { left, right, .. } => {
                qualify(left);
                qualify(right);
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => qualify(input),
            _ => unreachable!("not built by these tests"),
        }
    }

    fn column(name: &str) -> Expr {
        Expr::Column(name.into())
    }

    fn equal(left: &str, right: &str) -> Expr {
        Expr::BinaryOp {
            left: Box::new(column(left)),
            op: BinaryOp::Eq,
            right: Box::new(column(right)),
        }
    }

    #[test]
    fn a_comma_join_with_a_bare_equality_becomes_an_inner_hash_join() {
        let plan = bound(
            "SELECT c_name, o_orderdate FROM customer, orders WHERE c_custkey = o_custkey AND c_mktsegment = 'BUILDING' AND o_orderdate < 9000",
        );
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        // Projected columns bind to the join's names and keep their own.
        assert_eq!(
            columns,
            vec![
                Expr::Alias {
                    expr: Box::new(column("customer.c_name")),
                    name: "c_name".into()
                },
                Expr::Alias {
                    expr: Box::new(column("orders.o_orderdate")),
                    name: "o_orderdate".into()
                },
            ]
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            ..
        } = *input
        else {
            panic!("the filter went into the join, nothing stays above it");
        };
        assert_eq!(join_type, JoinType::Inner);
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        // Single-side conjuncts sit on their side, as bare names for the
        // scan to resolve.
        let LogicalPlan::Filter { predicate, input } = *left else {
            panic!("customer filter");
        };
        assert!(matches!(*input, LogicalPlan::Scan { .. }));
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(column("c_mktsegment")),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal(ScalarValue::Utf8("BUILDING".into()))),
            }
        );
        let LogicalPlan::Filter { predicate, .. } = *right else {
            panic!("orders filter");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Lt,
                ..
            }
        ));
    }

    #[test]
    fn using_wildcard_emits_the_join_key_once() {
        let plan = bound("SELECT * FROM customer c JOIN customer d USING (c_custkey)");
        let LogicalPlan::Project { columns, input } = plan else {
            panic!("projection");
        };
        assert_eq!(
            columns,
            vec![
                column("c.c_custkey"),
                column("c.c_name"),
                column("c.c_nationkey"),
                column("c.c_mktsegment"),
                column("d.c_name"),
                column("d.c_nationkey"),
                column("d.c_mktsegment"),
            ]
        );
        let LogicalPlan::Join {
            using_columns: Some(columns),
            ..
        } = *input
        else {
            panic!("USING metadata should survive binding");
        };
        assert_eq!(columns, vec!["c_custkey"]);
    }

    #[test]
    fn on_wildcard_keeps_both_join_keys() {
        let plan = bound("SELECT * FROM customer c JOIN customer d ON c.c_custkey = d.c_custkey");
        let LogicalPlan::Join {
            using_columns: None,
            ..
        } = plan
        else {
            panic!("ON join must not inherit USING wildcard metadata");
        };
    }

    #[test]
    fn qualified_wildcard_selects_only_the_named_relation() {
        let plan = bound("SELECT c.* FROM customer c JOIN customer d ON c.c_custkey = d.c_custkey");
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("qualified wildcard should remain a projection");
        };
        assert_eq!(columns.len(), 4);
        assert!(columns.iter().all(|column| matches!(
            column,
            Expr::Column(name) if name.starts_with("c.")
        )));
    }

    #[test]
    fn right_using_wildcard_uses_the_right_join_key() {
        let plan = bound("SELECT * FROM customer c RIGHT JOIN customer d USING (c_custkey)");
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert_eq!(columns[0], column("d.c_custkey"));
    }

    #[test]
    fn full_using_wildcard_coalesces_the_join_key() {
        let plan = bound("SELECT * FROM customer c FULL JOIN customer d USING (c_custkey)");
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "COALESCE"
                    && args == &vec![column("c.c_custkey"), column("d.c_custkey")]
        ));
    }

    #[test]
    fn a_three_way_comma_join_places_each_key_at_its_join_and_qualifies_the_aggregate() {
        let plan = bound(
            "SELECT l_orderkey, o_orderdate, SUM(l_quantity) AS q FROM customer, orders, lineitem WHERE c_custkey = o_custkey AND l_orderkey = o_orderkey AND c_mktsegment = 'BUILDING' GROUP BY l_orderkey, o_orderdate ORDER BY q DESC",
        );
        let LogicalPlan::Sort { input, order_by } = plan else {
            panic!("sort");
        };
        // Above the projection nothing is rewritten.
        assert_eq!(order_by, vec![(column("q"), false, true)]);
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("projection");
        };
        // The aggregate reference binds its argument like the aggregate.
        assert_eq!(
            columns[2],
            Expr::Alias {
                expr: Box::new(Expr::Function {
                    name: "SUM".into(),
                    args: vec![column("lineitem.l_quantity")]
                }),
                name: "q".into()
            }
        );
        let LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } = *input
        else {
            panic!("aggregate");
        };
        assert_eq!(
            group_by,
            vec![column("lineitem.l_orderkey"), column("orders.o_orderdate")]
        );
        assert!(matches!(
            &aggregates[0],
            AggregateExpr::Sum { expr, .. } if expr == &column("lineitem.l_quantity")
        ));
        let LogicalPlan::Join {
            left,
            right,
            condition,
            join_type,
            ..
        } = *input
        else {
            panic!("outer join");
        };
        assert_eq!(join_type, JoinType::Inner);
        assert_eq!(
            condition,
            Some(equal("orders.o_orderkey", "lineitem.l_orderkey"))
        );
        assert!(matches!(*right, LogicalPlan::Scan { .. }));
        let LogicalPlan::Join {
            left, condition, ..
        } = *left
        else {
            panic!("inner join");
        };
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        assert!(matches!(*left, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn mixed_conjuncts_stay_above_the_join_and_self_joins_need_qualifiers() {
        let plan = bound(
            "SELECT n1.n_name FROM nation n1, nation n2 WHERE n1.n_regionkey = n2.n_regionkey AND (n1.n_name = 1 OR n2.n_name = 2)",
        );
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        assert_eq!(columns, vec![column("n1.n_name")]);
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("the OR stays above the join");
        };
        assert!(matches!(predicate, Expr::Or(..)));
        let LogicalPlan::Join { condition, .. } = *input else {
            panic!("join");
        };
        assert_eq!(condition, Some(equal("n1.n_regionkey", "n2.n_regionkey")));
        let mut plan = sql_to_logical_plan(
            "SELECT n_name FROM nation n1, nation n2 WHERE n1.n_regionkey = n2.n_regionkey",
        )
        .unwrap();
        qualify(&mut plan);
        let error = bind(plan, &catalog()).unwrap_err().to_string();
        assert!(error.contains("'n_name' is ambiguous"), "{error}");
    }

    #[test]
    fn a_single_relation_is_left_as_written_and_an_unknown_relation_is_refused() {
        let single = "SELECT c_name FROM customer WHERE c_custkey = 1 ORDER BY c_name";
        let expected = format!("{:?}", {
            let mut plan = sql_to_logical_plan(single).unwrap();
            qualify(&mut plan);
            plan
        });
        assert_eq!(format!("{:?}", bound(single)), expected);
        // A relation the catalog does not hold fails at bind, naming it.
        let unknown = "SELECT a, b FROM unknown, customer WHERE a = c_custkey AND b = 2";
        let mut plan = sql_to_logical_plan(unknown).unwrap();
        qualify(&mut plan);
        let error = bind(plan, &catalog()).unwrap_err().to_string();
        assert_eq!(error, "execution: table 'lake.tpch.unknown' not found");
        // A catalog the session cannot see fails the same way as one that
        // does not exist, and the text does not tell the two apart.
        let mut plan = sql_to_logical_plan("SELECT c_name FROM customer").unwrap();
        qualify(&mut plan);
        let hidden = bind(plan, &catalog().restricted(Vec::<String>::new()))
            .unwrap_err()
            .to_string();
        let plan = sql_to_logical_plan("SELECT c_name FROM nowhere.tpch.customer").unwrap();
        let absent = bind(plan, &catalog()).unwrap_err().to_string();
        assert_eq!(hidden, "execution: catalog 'lake' not found");
        assert_eq!(absent, "execution: catalog 'nowhere' not found");
    }

    #[test]
    fn an_outer_join_filters_its_non_preserved_side_by_the_on_clause_and_refuses_residuals() {
        let plan = bound(
            "SELECT c_custkey, COUNT(o_orderkey) AS n FROM customer LEFT OUTER JOIN orders ON c_custkey = o_custkey AND o_orderdate <> 7 GROUP BY c_custkey",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Aggregate { input, .. } = *input else {
            panic!("aggregate");
        };
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            ..
        } = *input
        else {
            panic!("the join is the aggregate's input; nothing stays above it");
        };
        assert_eq!(join_type, JoinType::Left);
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        let LogicalPlan::Filter { predicate, .. } = *right else {
            panic!("orders filtered before the join");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Ne,
                ..
            }
        ));
        // The preserved side cannot be filtered by ON, and there is no
        // residual for an outer join.
        for sql in [
            "SELECT c_custkey FROM customer LEFT JOIN orders ON c_custkey = o_custkey AND c_nationkey <> 7",
            "SELECT c_custkey FROM customer LEFT JOIN orders ON c_custkey = o_custkey AND c_nationkey <> o_orderdate",
            "SELECT c_custkey FROM customer FULL JOIN orders ON c_custkey = o_custkey AND o_orderdate <> 7",
        ] {
            let mut plan = sql_to_logical_plan(sql).unwrap();
            qualify(&mut plan);
            let error = bind(plan, &catalog()).unwrap_err().to_string();
            assert!(error.contains("JOIN conditions"), "{sql}: {error}");
        }
        // An inner join filters a mixed conjunct above itself.
        let plan = bound(
            "SELECT c_custkey FROM customer JOIN orders ON c_custkey = o_custkey AND c_nationkey <> o_orderdate",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("residual above the inner join");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Ne,
                ..
            }
        ));
        assert!(matches!(
            *input,
            LogicalPlan::Join {
                join_type: JoinType::Inner,
                condition: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_correlated_exists_is_a_semi_join_on_the_correlation() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE o_orderdate > 1 AND EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity > 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
            residual,
        } = *input
        else {
            panic!("semi join at the top; the WHERE went into its left input");
        };
        assert_eq!(left_key, column("o_orderkey"));
        assert_eq!(right_key, column("*"));
        assert_eq!(residual, None);
        let LogicalPlan::Filter { input, .. } = *left else {
            panic!("orders filtered by the rest of the WHERE");
        };
        assert!(matches!(*input, LogicalPlan::Scan { .. }));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("the subquery projects its side of the correlation");
        };
        assert_eq!(columns, vec![column("l_orderkey")]);
        let LogicalPlan::Filter { predicate, input } = *input else {
            panic!("the subquery keeps its own predicate");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
        assert!(matches!(*input, LogicalPlan::Scan { .. }));
        // NOT EXISTS drops NULL keys on the build side.
        let plan = bound(
            "SELECT c_custkey FROM customer WHERE NOT EXISTS (SELECT * FROM orders WHERE o_custkey = c_custkey)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::AntiJoin {
            right, left_key, ..
        } = *input
        else {
            panic!("anti join");
        };
        assert_eq!(left_key, column("c_custkey"));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("key projection");
        };
        assert_eq!(columns, vec![column("o_custkey")]);
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("NULL keys filtered");
        };
        assert_eq!(predicate, Expr::IsNotNull(Box::new(column("o_custkey"))));
    }

    #[test]
    fn a_correlated_scalar_aggregate_is_a_join_on_the_grouped_aggregate() {
        let plan = bound(
            "SELECT o_orderkey FROM orders, customer WHERE c_custkey = o_custkey AND o_custkey > (SELECT avg(l_quantity) FROM lineitem WHERE l_orderkey = o_orderkey AND l_shipdate > 1 OFFSET 0)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("the comparison stays above the join");
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(column("orders.o_custkey")),
                op: BinaryOp::Gt,
                right: Box::new(column("__kaveon_scalar_0")),
            }
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            ..
        } = *input
        else {
            panic!("join with the subquery");
        };
        assert_eq!(join_type, JoinType::Inner);
        assert_eq!(
            condition,
            Some(equal("orders.o_orderkey", "__kaveon_corr_0"))
        );
        assert!(matches!(
            *left,
            LogicalPlan::Join {
                join_type: JoinType::Inner,
                ..
            }
        ));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("the subquery projects its value and its key");
        };
        assert!(matches!(&columns[0], Expr::Alias { name, .. } if name == "__kaveon_scalar_0"));
        assert_eq!(
            columns[1],
            Expr::Alias {
                expr: Box::new(column("l_orderkey")),
                name: "__kaveon_corr_0".into()
            }
        );
        let LogicalPlan::Aggregate {
            group_by, input, ..
        } = *input
        else {
            panic!("aggregate grouped by the correlation");
        };
        assert_eq!(group_by, vec![column("l_orderkey")]);
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("the subquery's own predicate stays");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
    }

    #[test]
    fn a_having_aggregate_inside_a_subquery_is_not_a_correlation() {
        let plan = bound(
            "SELECT o_orderkey FROM orders, lineitem WHERE o_orderkey = l_orderkey AND o_orderkey IN (SELECT l_orderkey FROM lineitem GROUP BY l_orderkey HAVING sum(l_quantity) > 300)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin { right, .. } = *input else {
            panic!("semi join");
        };
        let LogicalPlan::Project { input, .. } = *right else {
            panic!("subquery projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("the HAVING stays in the subquery");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
        assert!(matches!(*input, LogicalPlan::Aggregate { .. }));
    }

    #[test]
    fn a_correlated_count_preserves_unmatched_rows_with_zero() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE o_orderkey > (SELECT count(*) FROM lineitem WHERE l_orderkey = o_orderkey)",
        );
        let debug = format!("{plan:?}");
        assert!(debug.contains("join_type: Left"), "{debug}");
        assert!(debug.contains("COALESCE"), "{debug}");
    }

    #[test]
    fn correlations_the_join_cannot_carry_are_refused_with_their_reason() {
        for (sql, reason) in [
            (
                "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey <> o_orderkey)",
                "EXISTS correlated without an equality",
            ),
            (
                "SELECT o_orderkey FROM orders WHERE o_custkey IN (SELECT l_quantity FROM lineitem WHERE l_orderkey = o_orderkey)",
                "correlated IN subquery",
            ),
            (
                "SELECT o_orderkey FROM orders WHERE o_custkey > (SELECT avg(l_quantity) FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity > o_custkey)",
                "not supported under an aggregate",
            ),
            (
                "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem GROUP BY l_orderkey HAVING sum(l_quantity) > o_custkey)",
                "correlated HAVING",
            ),
            (
                "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity > o_custkey OFFSET 1)",
                "a positive correlated OFFSET requires equality correlations only",
            ),
            (
                "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem l WHERE l.l_orderkey = o_orderkey AND x.l_quantity > 1)",
                "belongs neither to the subquery nor to the enclosing query",
            ),
            (
                "SELECT c_custkey FROM customer WHERE EXISTS (SELECT * FROM orders WHERE o_custkey = c_custkey AND EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity > c_custkey))",
                "reaching past an IN or EXISTS subquery",
            ),
        ] {
            let mut plan = sql_to_logical_plan_for_binder(sql).unwrap();
            qualify(&mut plan);
            let error = bind(plan, &catalog()).unwrap_err().to_string();
            assert!(error.contains(reason), "{sql}: {error}");
        }
    }

    #[test]
    fn correlated_exists_positive_limit_is_decorrelated() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey LIMIT 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected outer projection")
        };
        assert!(matches!(*input, LogicalPlan::SemiJoin { .. }), "{input:?}");
    }

    #[test]
    fn correlated_exists_zero_limit_becomes_empty_relation() {
        let mut plan = sql_to_logical_plan_for_binder(
            "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey LIMIT 0)",
        )
        .unwrap();
        qualify(&mut plan);
        let plan = bind(plan, &catalog()).unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected outer projection")
        };
        let LogicalPlan::SemiJoin { right, .. } = *input else {
            panic!("expected semi join")
        };
        assert!(matches!(*right, LogicalPlan::Filter { .. }), "{right:?}");
    }

    #[test]
    fn correlated_exists_zero_offset_is_decorrelated() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey OFFSET 0)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected outer projection")
        };
        assert!(matches!(*input, LogicalPlan::SemiJoin { .. }), "{input:?}");
    }

    #[test]
    fn correlated_exists_positive_offset_uses_per_key_count() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey OFFSET 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected outer projection")
        };
        let LogicalPlan::SemiJoin { right, .. } = *input else {
            panic!("expected semi join")
        };
        assert!(matches!(
            *right,
            LogicalPlan::Filter {
                input,
                predicate: Expr::BinaryOp { op: BinaryOp::Gt, .. }
            } if matches!(*input, LogicalPlan::Aggregate { .. })
        ));
    }

    #[test]
    fn correlated_exists_positive_offset_groups_all_equality_keys() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity = o_custkey OFFSET 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected outer projection")
        };
        let LogicalPlan::SemiJoin {
            right, residual, ..
        } = *input
        else {
            panic!("expected semi join")
        };
        assert!(residual.is_some());
        let LogicalPlan::Project { input, .. } = *right else {
            panic!("expected projected count filter")
        };
        let LogicalPlan::Filter { input, .. } = *input else {
            panic!("expected count filter")
        };
        let LogicalPlan::Aggregate { group_by, .. } = *input else {
            panic!("expected per-key aggregate")
        };
        assert_eq!(group_by.len(), 2);
    }

    #[test]
    fn correlated_scalar_offset_with_residual_fails_closed() {
        let mut plan = sql_to_logical_plan_for_binder(
            "SELECT e.o_orderkey FROM orders e WHERE e.o_orderdate > (SELECT i.o_orderdate FROM orders i WHERE i.o_custkey = e.o_custkey AND i.o_orderdate > e.o_orderdate ORDER BY i.o_orderdate OFFSET 1 LIMIT 1)",
        )
        .unwrap();
        qualify(&mut plan);
        let error = bind(plan, &catalog())
            .expect_err("residual correlated scalar OFFSET requires Apply semantics");
        assert!(
            error
                .to_string()
                .contains("OFFSET inside a correlated subquery is not supported"),
            "unexpected diagnostic: {error}"
        );
    }

    #[test]
    fn correlated_scalar_offset_with_equality_uses_per_key_row_number() {
        let plan = bound(
            "SELECT e.o_orderkey FROM orders e WHERE e.o_orderdate > (SELECT i.o_orderdate FROM orders i WHERE i.o_custkey = e.o_custkey ORDER BY i.o_orderdate OFFSET 1 LIMIT 1)",
        );
        let debug = format!("{plan:?}");
        assert!(debug.contains("WindowFunction"), "{debug}");
        assert!(debug.contains("ROW_NUMBER"), "{debug}");
        assert!(debug.contains("o_custkey"), "{debug}");
        assert!(!debug.contains("Offset"), "paging must be per-key: {debug}");
        assert!(!debug.contains("Limit"), "paging must be per-key: {debug}");
    }

    #[test]
    fn a_correlated_in_with_the_projected_key_is_a_keyed_semi_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem l WHERE l_orderkey = o_orderkey)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            left_key,
            right_key,
            residual,
            ..
        } = *input
        else {
            panic!("correlated IN should lower to a semi join");
        };
        assert!(matches!(left_key, Expr::Column(name) if name == "o_orderkey"));
        assert!(matches!(right_key, Expr::Column(name) if name == "__kaveon_corr_0"));
        assert!(residual.is_none());
    }

    #[test]
    fn a_correlated_in_with_distinct_key_is_a_keyed_semi_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT DISTINCT l_orderkey FROM lineitem l WHERE l.l_orderkey = o_orderkey)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin { right_key, .. } = *input else {
            panic!("DISTINCT correlated IN should lower to a semi join");
        };
        assert_eq!(right_key, Expr::Column("__kaveon_corr_0".into()));
    }

    #[test]
    fn a_correlated_in_positive_limit_is_a_keyed_semi_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey LIMIT 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            right_key, right, ..
        } = *input
        else {
            panic!("positive LIMIT correlated IN should lower to a semi join");
        };
        assert_eq!(right_key, Expr::Column("__kaveon_corr_0".into()));
        assert!(matches!(*right, LogicalPlan::Project { .. }));
    }

    #[test]
    fn a_correlated_not_in_positive_limit_is_an_anti_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey NOT IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey LIMIT 1)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(*input, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn a_correlated_in_ordered_positive_limit_drops_order_before_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey ORDER BY l_quantity DESC LIMIT 1)",
        );
        let debug = format!("{plan:?}");
        assert!(matches!(
            plan,
            LogicalPlan::Project { input, .. }
                if matches!(*input, LogicalPlan::SemiJoin { .. })
        ));
        assert!(!debug.contains("Sort"), "{debug}");
        assert!(!debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn a_correlated_not_in_ordered_positive_limit_drops_order_before_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey NOT IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey ORDER BY l_quantity DESC LIMIT 1)",
        );
        assert!(matches!(
            plan,
            LogicalPlan::Project { input, .. }
                if matches!(*input, LogicalPlan::AntiJoin { .. })
        ));
    }

    #[test]
    fn a_correlated_in_zero_limit_is_an_empty_semi_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey LIMIT 0)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(*input, LogicalPlan::SemiJoin { .. }));
    }

    #[test]
    fn a_correlated_not_in_zero_limit_is_an_empty_anti_join() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey NOT IN (SELECT l_orderkey FROM lineitem l WHERE l.l_orderkey = o.o_orderkey LIMIT 0)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(*input, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn a_correlated_in_with_a_residual_uses_the_semi_join_residual_path() {
        let plan = bound(
            "SELECT o_orderkey FROM orders o WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem l WHERE l_orderkey = o_orderkey AND l_quantity > o_custkey)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            right_key,
            residual,
            right,
            ..
        } = *input
        else {
            panic!("correlated IN should lower to a residual semi join");
        };
        assert_eq!(right_key, column("__kaveon_corr_0"));
        assert!(matches!(
            residual,
            Some(Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            })
        ));
        let LogicalPlan::Project { columns, .. } = *right else {
            panic!("the subquery projects the key and residual column");
        };
        assert_eq!(columns.len(), 2);
    }

    /// Q21's EXISTS: one equality is the key, the non-equality the
    /// residual, whose subquery side is projected beside the key under a
    /// name of its own and whose enclosing side binds against the left.
    #[test]
    fn a_correlated_exists_with_a_non_equality_is_a_semi_join_with_a_residual() {
        let plan = bound(
            "SELECT l1.l_orderkey FROM orders, lineitem l1 WHERE o_orderkey = l1.l_orderkey AND EXISTS (SELECT * FROM lineitem l2 WHERE l2.l_orderkey = l1.l_orderkey AND l2.l_quantity <> l1.l_quantity AND l2.l_shipdate > 5)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
            residual,
        } = *input
        else {
            panic!("semi join");
        };
        assert_eq!(left_key, column("l1.l_orderkey"));
        assert_eq!(right_key, column("l2.l_orderkey"));
        assert_eq!(
            residual,
            Some(Expr::BinaryOp {
                left: Box::new(column("__kaveon_corr_0")),
                op: BinaryOp::Ne,
                right: Box::new(column("l1.l_quantity")),
            })
        );
        assert!(matches!(
            *left,
            LogicalPlan::Join {
                join_type: JoinType::Inner,
                ..
            }
        ));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("the subquery projects its key and the residual's column");
        };
        assert_eq!(
            columns,
            vec![
                column("l2.l_orderkey"),
                Expr::Alias {
                    expr: Box::new(column("l2.l_quantity")),
                    name: "__kaveon_corr_0".into()
                }
            ]
        );
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("the subquery keeps its own predicate");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
        // NOT EXISTS: the anti join, NULL keys dropped beneath the
        // projection; a second equality joins the residual.
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE NOT EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity = o_custkey AND l_shipdate > o_orderdate)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::AntiJoin {
            right,
            left_key,
            right_key,
            residual,
            ..
        } = *input
        else {
            panic!("anti join");
        };
        assert_eq!(left_key, column("o_orderkey"));
        assert_eq!(right_key, column("l_orderkey"));
        assert_eq!(
            residual,
            Some(Expr::And(
                Box::new(equal("__kaveon_corr_0", "o_custkey")),
                Box::new(Expr::BinaryOp {
                    left: Box::new(column("__kaveon_corr_1")),
                    op: BinaryOp::Gt,
                    right: Box::new(column("o_orderdate")),
                })
            ))
        );
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("key projection");
        };
        assert_eq!(columns.len(), 3);
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("NULL keys filtered");
        };
        assert_eq!(predicate, Expr::IsNotNull(Box::new(column("l_orderkey"))));
    }

    /// A residual whose subquery column rides through a projection is
    /// renamed with it.
    #[test]
    fn a_residual_follows_its_column_through_the_subquery_projection() {
        let plan = bound(
            "SELECT o_orderkey FROM orders WHERE o_orderkey IN (SELECT l_orderkey FROM lineitem) AND EXISTS (SELECT l_shipdate FROM lineitem WHERE l_orderkey = o_orderkey AND l_quantity > o_custkey)",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::SemiJoin {
            right, residual, ..
        } = *input
        else {
            panic!("the EXISTS semi join is outermost");
        };
        let Some(Expr::BinaryOp { left, .. }) = residual else {
            panic!("residual");
        };
        let Expr::Column(renamed) = *left else {
            panic!("the residual reads the projected column");
        };
        let LogicalPlan::Project { columns, .. } = *right else {
            panic!("the semi join projects key and residual column");
        };
        // The residual's column was projected out of the subquery's own
        // SELECT under a name, then again beside the key under another.
        assert!(columns.iter().any(|column| matches!(
            column,
            Expr::Alias { expr, name } if name == &renamed && matches!(expr.as_ref(), Expr::Column(inner) if inner.starts_with("__kaveon_corr_"))
        )));
    }

    #[test]
    fn an_explicit_join_condition_binds_and_the_where_routes_below_it() {
        let plan = bound(
            "SELECT c.c_name FROM customer c JOIN orders o ON c.c_custkey = o.o_custkey WHERE o_orderdate > 1 AND c_mktsegment = 'BUILDING'",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Join {
            left,
            right,
            condition,
            ..
        } = *input
        else {
            panic!("join with both filters below it");
        };
        assert_eq!(condition, Some(equal("c.c_custkey", "o.o_custkey")));
        assert!(matches!(*left, LogicalPlan::Filter { .. }));
        assert!(matches!(*right, LogicalPlan::Filter { .. }));
    }
}
