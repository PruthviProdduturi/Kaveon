use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::parser::parse_sql;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{
    BinaryOp, CastTarget, DateField, Expr, KaveonError, Result, WindowFrame, WindowFrameBound,
    WindowFrameUnits,
};
use sqlparser::ast;

#[derive(Debug, Clone, PartialEq)]
pub enum AggregateExpr {
    Count { expr: Expr, distinct: bool },
    Sum { expr: Expr, distinct: bool },
    Avg { expr: Expr, distinct: bool },
    Min(Expr),
    Max(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinDistribution {
    Partitioned,
    BroadcastRight,
}

#[derive(Debug)]
pub enum LogicalPlan {
    Scan {
        table: String,
        alias: Option<String>,
        columns: Option<Vec<String>>,
    },
    Join {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        join_type: JoinType,
        condition: Option<Expr>,
        distribution: JoinDistribution,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<LogicalPlan>,
        columns: Vec<Expr>,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<Expr>,
        aggregates: Vec<AggregateExpr>,
    },
    Sort {
        input: Box<LogicalPlan>,
        order_by: Vec<(Expr, bool)>,
    },
    Limit {
        input: Box<LogicalPlan>,
        count: usize,
    },
    Offset {
        input: Box<LogicalPlan>,
        count: usize,
    },
    Distinct {
        input: Box<LogicalPlan>,
    },
    Union {
        inputs: Vec<LogicalPlan>,
        all: bool,
    },
    Window {
        input: Box<LogicalPlan>,
        window_exprs: Vec<Expr>,
    },
    Intersect {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
    },
    Except {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
    },
    SemiJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        left_key: Expr,
        right_key: Expr,
    },
    AntiJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        left_key: Expr,
        right_key: Expr,
    },
}

pub fn sql_to_logical_plan(sql: &str) -> Result<LogicalPlan> {
    let stmts = parse_sql(sql)?;
    if stmts.is_empty() {
        return Err(sql_err("empty query"));
    }
    if stmts.len() > 1 {
        return Err(sql_err("only single statements are supported"));
    }
    statement_to_plan(&stmts[0])
}

fn statement_to_plan(stmt: &ast::Statement) -> Result<LogicalPlan> {
    match stmt {
        ast::Statement::Query(query) => query_to_plan(query, &Lowering::default()),
        _ => Err(sql_err("only SELECT queries are supported")),
    }
}

/// What a query level lowers against: the CTEs in scope, and the
/// statement-wide counter that names scalar subquery columns.
#[derive(Clone, Default)]
struct Lowering {
    ctes: HashMap<String, ast::Query>,
    scalars: Rc<Cell<usize>>,
}

impl Lowering {
    fn scalar_name(&self) -> String {
        let ordinal = self.scalars.get();
        self.scalars.set(ordinal + 1);
        format!("__kaveon_scalar_{ordinal}")
    }
}

fn query_to_plan(query: &ast::Query, parent: &Lowering) -> Result<LogicalPlan> {
    let mut context = parent.clone();
    if let Some(with) = &query.with {
        if with.recursive {
            return Err(sql_err("recursive CTEs are not supported"));
        }
        for cte in &with.cte_tables {
            let cte_name = cte.alias.name.value.to_lowercase();
            context.ctes.insert(cte_name, *cte.query.clone());
        }
    }
    let ctes = &context;

    // A single SELECT hands back the bindings its aggregate lowering made,
    // so an ORDER BY that repeats a lowered expression (`ORDER BY a - a % 60`
    // beside `GROUP BY a - a % 60`) resolves to the same column.
    let (plan, bindings) = match query.body.as_ref() {
        ast::SetExpr::Select(select) => select_to_plan_with_bindings(select, ctes)?,
        body => (set_expr_to_plan(body, ctes)?, Vec::new()),
    };

    let (plan, visible) = match &query.order_by {
        Some(ob) => build_order_by(plan, &ob.exprs, &bindings)?,
        None => (plan, None),
    };
    let plan = build_limit_offset(plan, &query.limit, &query.offset)?;
    // ORDER BY on a column the query does not select: the column rode
    // along through the sort and limit and is dropped here.
    let plan = match visible {
        Some(columns) => LogicalPlan::Project {
            input: Box::new(plan),
            columns,
        },
        None => plan,
    };

    Ok(plan)
}

fn set_expr_to_plan(body: &ast::SetExpr, ctes: &Lowering) -> Result<LogicalPlan> {
    match body {
        ast::SetExpr::Select(select) => select_to_plan(select, ctes),
        ast::SetExpr::SetOperation {
            op,
            left,
            right,
            set_quantifier,
        } => {
            let left_plan = set_expr_to_plan(left, ctes)?;
            let right_plan = set_expr_to_plan(right, ctes)?;
            match op {
                ast::SetOperator::Union => {
                    let all = matches!(set_quantifier, ast::SetQuantifier::All);
                    let plan = LogicalPlan::Union {
                        inputs: vec![left_plan, right_plan],
                        all,
                    };
                    if all {
                        Ok(plan)
                    } else {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    }
                }
                ast::SetOperator::Intersect => {
                    if matches!(set_quantifier, ast::SetQuantifier::All) {
                        return Err(sql_err("INTERSECT ALL is unsupported"));
                    }
                    let plan = LogicalPlan::Intersect {
                        left: Box::new(left_plan),
                        right: Box::new(right_plan),
                    };
                    if matches!(set_quantifier, ast::SetQuantifier::All) {
                        Ok(plan)
                    } else {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    }
                }
                ast::SetOperator::Except => {
                    if matches!(set_quantifier, ast::SetQuantifier::All) {
                        return Err(sql_err("EXCEPT ALL is unsupported"));
                    }
                    let plan = LogicalPlan::Except {
                        left: Box::new(left_plan),
                        right: Box::new(right_plan),
                    };
                    if matches!(set_quantifier, ast::SetQuantifier::All) {
                        Ok(plan)
                    } else {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    }
                }
            }
        }
        ast::SetExpr::Query(q) => query_to_plan(q, ctes),
        _ => Err(sql_err("unsupported query form")),
    }
}

fn select_to_plan(select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    Ok(select_to_plan_with_bindings(select, ctes)?.0)
}

fn select_to_plan_with_bindings(
    select: &ast::Select,
    ctes: &Lowering,
) -> Result<(LogicalPlan, ExpressionBindings)> {
    let plan = build_from_clause(select, ctes)?;
    let plan = build_where(plan, select, ctes)?;

    let has_aggregates = select.projection.iter().any(contains_aggregate_select_item)
        || select.having.as_ref().is_some_and(contains_aggregate_expr)
        || matches!(&select.group_by, ast::GroupByExpr::Expressions(exprs, _) if !exprs.is_empty());

    let plan = if has_aggregates {
        build_aggregate(plan, select)?
    } else {
        plan
    };

    let (plan, having_scalars) = build_having(plan, select, ctes)?;

    let window_exprs = collect_window_exprs(select)?;
    let plan = if window_exprs.is_empty() {
        plan
    } else {
        LogicalPlan::Window {
            input: Box::new(plan),
            window_exprs,
        }
    };

    let plan = build_projection(plan, select, has_aggregates)?;

    let plan = if matches!(select.distinct, Some(ast::Distinct::Distinct)) {
        LogicalPlan::Distinct {
            input: Box::new(plan),
        }
    } else {
        plan
    };

    // Every aggregate is lowered to a named column when something above
    // the aggregate must find it by name: a HAVING that compares with a
    // scalar subquery references the aggregate's output across the join
    // that carries the subquery's value, and a projection that computes
    // with an aggregate (`0.2 * avg(x)`) evaluates the expression over the
    // aggregate's output rather than the aggregate itself.
    let force = !having_scalars.is_empty() || select.projection.iter().any(computes_with_aggregate);
    let (plan, bindings) = lower_aggregate_expressions(plan, force)?;
    Ok((attach_having_scalars(plan, having_scalars), bindings))
}

/// A projected item that is an expression over an aggregate, rather than
/// an aggregate (or an aliased aggregate) itself.
fn computes_with_aggregate(item: &ast::SelectItem) -> bool {
    let expr = match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => expr,
        _ => return false,
    };
    let expr = match expr {
        ast::Expr::Nested(inner) => inner.as_ref(),
        other => other,
    };
    !matches!(expr, ast::Expr::Function(_)) && contains_aggregate_expr(expr)
}

/// The scalar subqueries of a HAVING join the aggregate's output: one
/// single-row relation per subquery, cross-joined below the HAVING filter.
fn attach_having_scalars(plan: LogicalPlan, scalars: Vec<LogicalPlan>) -> LogicalPlan {
    if scalars.is_empty() {
        return plan;
    }
    match plan {
        // The first filter from the top is the HAVING: WHERE sits below
        // the aggregate.
        LogicalPlan::Filter { input, predicate } => {
            let mut input = *input;
            for scalar in scalars {
                input = LogicalPlan::Join {
                    left: Box::new(input),
                    right: Box::new(scalar),
                    join_type: JoinType::Cross,
                    condition: None,
                    distribution: JoinDistribution::Partitioned,
                };
            }
            LogicalPlan::Filter {
                input: Box::new(input),
                predicate: bind_count_star(predicate),
            }
        }
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input: Box::new(attach_having_scalars(*input, scalars)),
            columns,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(attach_having_scalars(*input, scalars)),
        },
        LogicalPlan::Window {
            input,
            window_exprs,
        } => LogicalPlan::Window {
            input: Box::new(attach_having_scalars(*input, scalars)),
            window_exprs,
        },
        other => other,
    }
}

/// `COUNT(*)` has no argument to lower, so a HAVING that carries it
/// across the scalar join names its output column directly.
fn bind_count_star(expr: Expr) -> Expr {
    let bind = |expr: Box<Expr>| Box::new(bind_count_star(*expr));
    match expr {
        Expr::Function { name, args }
            if name == "COUNT" && matches!(args.as_slice(), [Expr::Star]) =>
        {
            Expr::Column("count_*".into())
        }
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: bind(left),
            op,
            right: bind(right),
        },
        Expr::And(left, right) => Expr::And(bind(left), bind(right)),
        Expr::Or(left, right) => Expr::Or(bind(left), bind(right)),
        Expr::Not(inner) => Expr::Not(bind(inner)),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: bind(expr),
            low: bind(low),
            high: bind(high),
            negated,
        },
        other => other,
    }
}

fn build_from_clause(select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    if select.from.is_empty() {
        return Err(sql_err("SELECT requires a FROM clause"));
    }
    let mut inputs = select.from.iter();
    let first = inputs.next().expect("FROM was checked as non-empty");
    let mut plan = table_factor_to_plan(&first.relation, ctes)?;
    plan = apply_joins(plan, &first.joins, ctes)?;
    for input in inputs {
        let right = apply_joins(
            table_factor_to_plan(&input.relation, ctes)?,
            &input.joins,
            ctes,
        )?;
        plan = LogicalPlan::Join {
            left: Box::new(plan),
            right: Box::new(right),
            join_type: JoinType::Cross,
            condition: None,
            distribution: JoinDistribution::Partitioned,
        };
    }
    Ok(plan)
}

fn table_factor_to_plan(factor: &ast::TableFactor, ctes: &Lowering) -> Result<LogicalPlan> {
    match factor {
        ast::TableFactor::Table { name, alias, .. } => {
            let table_name = name.to_string();
            let alias_name = alias.as_ref().map(|a| a.name.value.clone());
            let lookup = alias_name.as_deref().unwrap_or(&table_name).to_lowercase();
            if let Some(cte_query) = ctes
                .ctes
                .get(&table_name.to_lowercase())
                .or_else(|| ctes.ctes.get(&lookup))
            {
                let mut plan = query_to_plan(
                    &ast::Query {
                        with: None,
                        body: cte_query.body.clone(),
                        order_by: cte_query.order_by.clone(),
                        limit: cte_query.limit.clone(),
                        offset: cte_query.offset.clone(),
                        ..cte_query.clone()
                    },
                    ctes,
                )?;
                if alias_name.is_some()
                    && let LogicalPlan::Scan { ref mut alias, .. } = plan
                {
                    *alias = alias_name.clone();
                }
                Ok(plan)
            } else {
                Ok(LogicalPlan::Scan {
                    table: table_name,
                    alias: alias_name,
                    columns: None,
                })
            }
        }
        ast::TableFactor::Derived {
            subquery, alias, ..
        } => {
            let plan = query_to_plan(subquery, ctes)?;
            let _ = alias;
            Ok(plan)
        }
        _ => Err(sql_err(
            "only table references and subqueries are supported in FROM",
        )),
    }
}

fn apply_joins(mut left: LogicalPlan, joins: &[ast::Join], ctes: &Lowering) -> Result<LogicalPlan> {
    for join in joins {
        let right = table_factor_to_plan(&join.relation, ctes)?;
        let (join_type, constraint) = match &join.join_operator {
            ast::JoinOperator::Inner(c) => (JoinType::Inner, Some(c)),
            ast::JoinOperator::LeftOuter(c) => (JoinType::Left, Some(c)),
            ast::JoinOperator::RightOuter(c) => (JoinType::Right, Some(c)),
            ast::JoinOperator::FullOuter(c) => (JoinType::Full, Some(c)),
            ast::JoinOperator::CrossJoin => (JoinType::Cross, None),
            other => return Err(sql_err(format!("unsupported join type: {other:?}"))),
        };
        let condition = match constraint {
            None | Some(ast::JoinConstraint::None) => None,
            Some(ast::JoinConstraint::On(expr)) => Some(ast_expr_to_expr(expr)?),
            Some(other) => return Err(sql_err(format!("unsupported join constraint: {other:?}"))),
        };
        if join_type != JoinType::Cross && condition.is_none() {
            return Err(sql_err("non-cross joins require an ON condition"));
        }
        left = LogicalPlan::Join {
            left: Box::new(left),
            right: Box::new(right),
            join_type,
            condition,
            distribution: JoinDistribution::Partitioned,
        };
    }
    Ok(left)
}

fn build_where(plan: LogicalPlan, select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    match &select.selection {
        None => Ok(plan),
        Some(expr) => {
            let mut plan = plan;
            let mut remaining = Vec::new();
            extract_subquery_predicates(expr, &mut plan, &mut remaining, ctes)?;
            if remaining.is_empty() {
                Ok(plan)
            } else {
                let predicate = remaining
                    .into_iter()
                    .reduce(|a, b| Expr::And(Box::new(a), Box::new(b)))
                    .unwrap();
                Ok(LogicalPlan::Filter {
                    input: Box::new(plan),
                    predicate,
                })
            }
        }
    }
}

fn extract_subquery_predicates(
    expr: &ast::Expr,
    plan: &mut LogicalPlan,
    remaining: &mut Vec<Expr>,
    ctes: &Lowering,
) -> Result<()> {
    match expr {
        ast::Expr::BinaryOp {
            left,
            op: ast::BinaryOperator::And,
            right,
        } => {
            extract_subquery_predicates(left, plan, remaining, ctes)?;
            extract_subquery_predicates(right, plan, remaining, ctes)?;
            Ok(())
        }
        ast::Expr::InSubquery {
            expr: lhs,
            subquery,
            negated,
        } => {
            let left_key = ast_expr_to_expr(lhs)?;
            let sub_plan = query_to_plan(subquery, ctes)?;
            validate_uncorrelated(&sub_plan)?;
            // The physical operator binds the sole projected output by position.
            let right_key = Expr::Column("*".into());
            let current = std::mem::replace(
                plan,
                LogicalPlan::Scan {
                    table: String::new(),
                    alias: None,
                    columns: None,
                },
            );
            if *negated {
                *plan = LogicalPlan::AntiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                };
            } else {
                *plan = LogicalPlan::SemiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                };
            }
            Ok(())
        }
        ast::Expr::Exists { subquery, negated } => {
            let sub_plan = query_to_plan(subquery, ctes)?;
            validate_uncorrelated(&sub_plan)?;
            // Existence depends on row cardinality, including NULL-valued rows.
            // The independently planned RHS cannot resolve correlated outer columns.
            let right_key = Expr::Literal(ScalarValue::Int64(1));
            let left_key = right_key.clone();
            let current = std::mem::replace(
                plan,
                LogicalPlan::Scan {
                    table: String::new(),
                    alias: None,
                    columns: None,
                },
            );
            if *negated {
                *plan = LogicalPlan::AntiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                };
            } else {
                *plan = LogicalPlan::SemiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                };
            }
            Ok(())
        }
        ast::Expr::Nested(inner) => extract_subquery_predicates(inner, plan, remaining, ctes),
        other => {
            let rewritten = replace_scalar_subqueries(other, ctes, &mut |scalar| {
                let current = std::mem::replace(
                    plan,
                    LogicalPlan::Scan {
                        table: String::new(),
                        alias: None,
                        columns: None,
                    },
                );
                *plan = LogicalPlan::Join {
                    left: Box::new(current),
                    right: Box::new(scalar),
                    join_type: JoinType::Cross,
                    condition: None,
                    distribution: JoinDistribution::Partitioned,
                };
            })?;
            remaining.push(ast_expr_to_expr(&rewritten)?);
            Ok(())
        }
    }
}

/// Each scalar subquery in `expr` becomes one column of a single-row
/// relation, handed to `sink` to join into the query, and the expression
/// refers to that column. `x < (SELECT avg(y) FROM t)` is `x <
/// __kaveon_scalar_0` beside a cross join with the one-row aggregate; the
/// binder later turns a subquery correlated with the query into a join on
/// the correlation. Only subqueries in comparison positions are lowered;
/// one nested deeper keeps its error.
fn replace_scalar_subqueries(
    expr: &ast::Expr,
    ctes: &Lowering,
    sink: &mut dyn FnMut(LogicalPlan),
) -> Result<ast::Expr> {
    Ok(match expr {
        ast::Expr::Subquery(query) => {
            let name = ctes.scalar_name();
            sink(scalar_subquery_plan(query_to_plan(query, ctes)?, &name)?);
            ast::Expr::Identifier(ast::Ident::new(name))
        }
        ast::Expr::BinaryOp { left, op, right } => ast::Expr::BinaryOp {
            left: Box::new(replace_scalar_subqueries(left, ctes, sink)?),
            op: op.clone(),
            right: Box::new(replace_scalar_subqueries(right, ctes, sink)?),
        },
        ast::Expr::Nested(inner) => {
            ast::Expr::Nested(Box::new(replace_scalar_subqueries(inner, ctes, sink)?))
        }
        ast::Expr::UnaryOp { op, expr } => ast::Expr::UnaryOp {
            op: op.clone(),
            expr: Box::new(replace_scalar_subqueries(expr, ctes, sink)?),
        },
        ast::Expr::Between {
            expr,
            negated,
            low,
            high,
        } => ast::Expr::Between {
            expr: Box::new(replace_scalar_subqueries(expr, ctes, sink)?),
            negated: *negated,
            low: Box::new(replace_scalar_subqueries(low, ctes, sink)?),
            high: Box::new(replace_scalar_subqueries(high, ctes, sink)?),
        },
        other => other.clone(),
    })
}

/// The subquery's plan as a single-row relation whose one column is
/// `name`: an ungrouped aggregate (possibly under HAVING) selecting one
/// expression. Anything else could yield several rows, which a scalar
/// position cannot take.
fn scalar_subquery_plan(plan: LogicalPlan, name: &str) -> Result<LogicalPlan> {
    validate_uncorrelated(&plan)?;
    let LogicalPlan::Project { input, columns } = plan else {
        return Err(sql_err("a scalar subquery must select exactly one column"));
    };
    let [column] = columns.as_slice() else {
        return Err(sql_err("a scalar subquery must select exactly one column"));
    };
    if matches!(column, Expr::Star) {
        return Err(sql_err("a scalar subquery must select exactly one column"));
    }
    fn single_row(plan: &LogicalPlan) -> bool {
        match plan {
            LogicalPlan::Aggregate { group_by, .. } => group_by.is_empty(),
            LogicalPlan::Filter { input, .. } => single_row(input),
            _ => false,
        }
    }
    if !single_row(&input) {
        return Err(sql_err(
            "a scalar subquery must be an ungrouped aggregate, so that it yields one row",
        ));
    }
    let expr = match column {
        Expr::Alias { expr, .. } => (**expr).clone(),
        other => other.clone(),
    };
    Ok(LogicalPlan::Project {
        input,
        columns: vec![Expr::Alias {
            expr: Box::new(expr),
            name: name.to_owned(),
        }],
    })
}

fn build_aggregate(plan: LogicalPlan, select: &ast::Select) -> Result<LogicalPlan> {
    let group_by_ast = match &select.group_by {
        ast::GroupByExpr::Expressions(exprs, _) => exprs.clone(),
        _ => Vec::new(),
    };
    let group_by: Vec<Expr> = group_by_ast
        .iter()
        .map(ast_expr_to_expr)
        .collect::<Result<_>>()?;

    let mut aggregates = Vec::new();
    for item in &select.projection {
        collect_aggregates_from_select_item(item, &mut aggregates)?;
    }
    // HAVING may aggregate what the projection does not (`SELECT k ...
    // GROUP BY k HAVING SUM(x) > 300`); an aggregate it shares with the
    // projection is computed once.
    if let Some(having) = &select.having {
        let mut from_having = Vec::new();
        collect_aggregates_from_ast_expr(having, &mut from_having)?;
        for aggregate in from_having {
            if !aggregates.contains(&aggregate) {
                aggregates.push(aggregate);
            }
        }
    }

    // GROUP BY over plain columns with nothing to aggregate is DISTINCT over
    // those columns.
    if aggregates.is_empty()
        && !group_by.is_empty()
        && group_by.iter().all(|expr| matches!(expr, Expr::Column(_)))
    {
        return Ok(LogicalPlan::Distinct {
            input: Box::new(LogicalPlan::Project {
                input: Box::new(plan),
                columns: group_by,
            }),
        });
    }

    // COUNT(DISTINCT x) [GROUP BY k] is COUNT(x) over the distinct (k, x)
    // rows: the distinct step partitions across the workers by value, so no
    // single task has to hold every distinct value.
    if let [
        AggregateExpr::Count {
            expr: Expr::Column(column),
            distinct: true,
        },
    ] = aggregates.as_slice()
        && group_by.iter().all(|expr| matches!(expr, Expr::Column(_)))
    {
        let counted = Expr::Column(column.clone());
        let mut columns = group_by.clone();
        if !columns.contains(&counted) {
            columns.push(counted.clone());
        }
        return Ok(LogicalPlan::Aggregate {
            input: Box::new(LogicalPlan::Distinct {
                input: Box::new(LogicalPlan::Project {
                    input: Box::new(plan),
                    columns,
                }),
            }),
            group_by,
            aggregates: vec![AggregateExpr::Count {
                expr: counted,
                distinct: false,
            }],
        });
    }

    Ok(LogicalPlan::Aggregate {
        input: Box::new(plan),
        group_by,
        aggregates,
    })
}

/// The HAVING filter, and the single-row relations of its scalar
/// subqueries, which join the aggregate's output once the aggregate is
/// lowered (see `attach_having_scalars`).
fn build_having(
    plan: LogicalPlan,
    select: &ast::Select,
    ctes: &Lowering,
) -> Result<(LogicalPlan, Vec<LogicalPlan>)> {
    match &select.having {
        None => Ok((plan, Vec::new())),
        Some(expr) => {
            let mut scalars = Vec::new();
            let rewritten =
                replace_scalar_subqueries(expr, ctes, &mut |scalar| scalars.push(scalar))?;
            let predicate = ast_expr_to_expr(&rewritten)?;
            Ok((
                LogicalPlan::Filter {
                    input: Box::new(plan),
                    predicate,
                },
                scalars,
            ))
        }
    }
}

type ExpressionBindings = Vec<(Expr, Expr)>;

/// Compute aggregate/group expressions once below aggregation, then bind
/// references above it to the resulting columns. Both execution paths receive
/// column-only aggregate arguments while preserving the original SQL types.
fn lower_aggregate_expressions(
    plan: LogicalPlan,
    force: bool,
) -> Result<(LogicalPlan, ExpressionBindings)> {
    match plan {
        LogicalPlan::Project { input, columns } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let columns = columns
                .into_iter()
                .map(|e| replace_bound_expression(e, &bindings))
                .collect();
            Ok((
                LogicalPlan::Project {
                    input: Box::new(input),
                    columns,
                },
                bindings,
            ))
        }
        LogicalPlan::Filter { input, predicate } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let predicate = replace_bound_expression(predicate, &bindings);
            Ok((
                LogicalPlan::Filter {
                    input: Box::new(input),
                    predicate,
                },
                bindings,
            ))
        }
        LogicalPlan::Distinct { input } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            Ok((
                LogicalPlan::Distinct {
                    input: Box::new(input),
                },
                bindings,
            ))
        }
        LogicalPlan::Window {
            input,
            window_exprs,
        } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let window_exprs = window_exprs
                .into_iter()
                .map(|e| replace_bound_expression(e, &bindings))
                .collect();
            Ok((
                LogicalPlan::Window {
                    input: Box::new(input),
                    window_exprs,
                },
                bindings,
            ))
        }
        LogicalPlan::Aggregate {
            input,
            mut group_by,
            mut aggregates,
        } => {
            let complex = group_by.iter().any(|e| !matches!(e, Expr::Column(_)))
                || aggregates.iter().any(|a| {
                    let e = match a {
                        AggregateExpr::Count { expr, .. }
                        | AggregateExpr::Sum { expr, .. }
                        | AggregateExpr::Avg { expr, .. }
                        | AggregateExpr::Min(expr)
                        | AggregateExpr::Max(expr) => expr,
                    };
                    !matches!(e, Expr::Column(_) | Expr::Star)
                });
            if !complex && !force {
                return Ok((
                    LogicalPlan::Aggregate {
                        input,
                        group_by,
                        aggregates,
                    },
                    vec![],
                ));
            }
            let mut projection = Vec::new();
            let mut bindings = Vec::new();
            for (i, expr) in group_by.iter_mut().enumerate() {
                let name = match expr {
                    Expr::Column(name) => name.clone(),
                    _ => format!("__kaveon_group_{i}"),
                };
                projection.push(Expr::Alias {
                    expr: Box::new(expr.clone()),
                    name: name.clone(),
                });
                let column = Expr::Column(name);
                bindings.push((expr.clone(), column.clone()));
                *expr = column;
            }
            for (i, aggregate) in aggregates.iter_mut().enumerate() {
                let (function, expr) = match aggregate {
                    AggregateExpr::Count { expr, .. } => ("COUNT", expr),
                    AggregateExpr::Sum { expr, .. } => ("SUM", expr),
                    AggregateExpr::Avg { expr, .. } => ("AVG", expr),
                    AggregateExpr::Min(expr) => ("MIN", expr),
                    AggregateExpr::Max(expr) => ("MAX", expr),
                };
                if matches!(expr, Expr::Star) {
                    continue;
                }
                let original = Expr::Function {
                    name: function.into(),
                    args: vec![expr.clone()],
                };
                let name = format!("__kaveon_arg_{i}");
                projection.push(Expr::Alias {
                    expr: Box::new(expr.clone()),
                    name: name.clone(),
                });
                *expr = Expr::Column(name.clone());
                bindings.push((
                    original,
                    Expr::Column(format!("{}_{name}", function.to_lowercase())),
                ));
            }
            Ok((
                LogicalPlan::Aggregate {
                    input: Box::new(LogicalPlan::Project {
                        input,
                        columns: projection,
                    }),
                    group_by,
                    aggregates,
                },
                bindings,
            ))
        }
        other => Ok((other, vec![])),
    }
}

fn replace_bound_expression(expr: Expr, bindings: &ExpressionBindings) -> Expr {
    if let Some((_, replacement)) = bindings.iter().find(|(original, _)| original == &expr) {
        return replacement.clone();
    }
    match expr {
        Expr::Alias { expr, name } => Expr::Alias {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            name,
        },
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            data_type,
        },
        Expr::Extract { expr, field } => Expr::Extract {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            field,
        },
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(replace_bound_expression(*left, bindings)),
            op,
            right: Box::new(replace_bound_expression(*right, bindings)),
        },
        Expr::IsNull(expr) => Expr::IsNull(Box::new(replace_bound_expression(*expr, bindings))),
        Expr::IsNotNull(expr) => {
            Expr::IsNotNull(Box::new(replace_bound_expression(*expr, bindings)))
        }
        Expr::Not(expr) => Expr::Not(Box::new(replace_bound_expression(*expr, bindings))),
        Expr::And(a, b) => Expr::And(
            Box::new(replace_bound_expression(*a, bindings)),
            Box::new(replace_bound_expression(*b, bindings)),
        ),
        Expr::Or(a, b) => Expr::Or(
            Box::new(replace_bound_expression(*a, bindings)),
            Box::new(replace_bound_expression(*b, bindings)),
        ),
        Expr::Function { name, args } => Expr::Function {
            name,
            args: args
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
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
                .map(|e| replace_bound_expression(e, bindings))
                .collect(),
            partition_by: partition_by
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
                .collect(),
            order_by: order_by
                .into_iter()
                .map(|(e, b)| (replace_bound_expression(e, bindings), b))
                .collect(),
            frame,
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => Expr::Case {
            operand: operand.map(|e| Box::new(replace_bound_expression(*e, bindings))),
            when_then: when_then
                .into_iter()
                .map(|(a, b)| {
                    (
                        replace_bound_expression(a, bindings),
                        replace_bound_expression(b, bindings),
                    )
                })
                .collect(),
            else_expr: else_expr.map(|e| Box::new(replace_bound_expression(*e, bindings))),
        },
        other => other,
    }
}

fn build_projection(
    plan: LogicalPlan,
    select: &ast::Select,
    has_aggregates: bool,
) -> Result<LogicalPlan> {
    let mut columns = Vec::new();
    let mut is_star = false;

    for item in &select.projection {
        match item {
            ast::SelectItem::UnnamedExpr(expr) => {
                columns.push(ast_expr_to_expr(expr)?);
            }
            ast::SelectItem::ExprWithAlias { expr, alias } => {
                let e = ast_expr_to_expr(expr)?;
                columns.push(Expr::Alias {
                    expr: Box::new(e),
                    name: alias.value.clone(),
                });
            }
            ast::SelectItem::Wildcard(_) => {
                is_star = true;
                columns.push(Expr::Star);
            }
            ast::SelectItem::QualifiedWildcard(_, _) => {
                is_star = true;
                columns.push(Expr::Star);
            }
        }
    }

    if is_star && columns.len() == 1 && !has_aggregates {
        return Ok(plan);
    }

    Ok(LogicalPlan::Project {
        input: Box::new(plan),
        columns,
    })
}

/// The sorted plan and, when a key had to be carried through the
/// projection, the visible columns to re-project at the top.
fn build_order_by(
    plan: LogicalPlan,
    order_by: &[ast::OrderByExpr],
    bindings: &ExpressionBindings,
) -> Result<(LogicalPlan, Option<Vec<Expr>>)> {
    if order_by.is_empty() {
        return Ok((plan, None));
    }
    let mut items = Vec::new();
    for ob in order_by {
        let expr = replace_bound_expression(ast_expr_to_expr(&ob.expr)?, bindings);
        let asc = ob.asc.unwrap_or(true);
        items.push((expr, asc));
    }
    let (plan, visible) = bind_order_keys_to_projection(plan, &mut items);
    Ok((
        LogicalPlan::Sort {
            input: Box::new(plan),
            order_by: items,
        },
        visible,
    ))
}

/// An ORDER BY expression that repeats a select item — `ORDER BY COUNT(*)`
/// beside `SELECT k, COUNT(*)` — orders by that item's output column. An
/// item without a name receives one (`expr_<position>`) so the key can
/// name it.
fn bind_order_keys_to_projection(
    plan: LogicalPlan,
    keys: &mut [(Expr, bool)],
) -> (LogicalPlan, Option<Vec<Expr>>) {
    let LogicalPlan::Project { input, mut columns } = plan else {
        return (plan, None);
    };
    if columns.iter().any(|column| matches!(column, Expr::Star)) {
        return (LogicalPlan::Project { input, columns }, None);
    }
    let output_names = |columns: &[Expr]| {
        columns
            .iter()
            .filter_map(|column| match column {
                Expr::Alias { name, .. } => Some(name.clone()),
                Expr::Column(name) => Some(name.rsplit('.').next().unwrap_or(name).to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let visible = columns.clone();
    let mut hidden = Vec::new();
    for (key, _) in keys.iter_mut() {
        if let Expr::Column(name) = key {
            // `ORDER BY u.country` when the query selects `u.country AS
            // user_country` orders by that output column.
            let aliased = columns.iter().find_map(|column| match column {
                Expr::Alias { expr, name: alias }
                    if matches!(expr.as_ref(), Expr::Column(selected) if selected == name) =>
                {
                    Some(alias.clone())
                }
                _ => None,
            });
            if let Some(alias) = aliased {
                *key = Expr::Column(alias);
                continue;
            }
            // A column the query does not select rides along through the
            // sort; the caller re-projects the visible columns above. A
            // qualified key is selected only by the same qualified column
            // (`t.country` does not stand in for `u.country`); a bare key
            // by any column of that name.
            let selected_exactly = columns
                .iter()
                .any(|column| matches!(column, Expr::Column(selected) if selected == name));
            let bare = name.rsplit('.').next().unwrap_or(name).to_owned();
            let selected_bare = !name.contains('.') && output_names(&columns).contains(&bare);
            if !selected_exactly && !selected_bare && !hidden.contains(name) {
                hidden.push(name.clone());
                columns.push(Expr::Column(name.clone()));
            }
            continue;
        }
        if matches!(key, Expr::Literal(_)) {
            continue;
        }
        let position = columns.iter().position(|column| match column {
            Expr::Alias { expr, .. } => expr.as_ref() == key,
            other => other == key,
        });
        let Some(position) = position else {
            continue;
        };
        let name = match &columns[position] {
            Expr::Alias { name, .. } => name.clone(),
            other => {
                let name = format!("expr_{position}");
                columns[position] = Expr::Alias {
                    expr: Box::new(other.clone()),
                    name: name.clone(),
                };
                name
            }
        };
        *key = Expr::Column(name);
    }
    let visible = (!hidden.is_empty()).then(|| {
        visible
            .iter()
            .enumerate()
            .map(|(position, column)| match column {
                Expr::Alias { name, .. } => Expr::Column(name.clone()),
                Expr::Column(name) => Expr::Column(name.clone()),
                other => {
                    // An unnamed expression is addressed by the name it
                    // received in the extended projection.
                    let name = format!("expr_{position}");
                    columns[position] = Expr::Alias {
                        expr: Box::new(other.clone()),
                        name: name.clone(),
                    };
                    Expr::Column(name)
                }
            })
            .collect::<Vec<_>>()
    });
    (LogicalPlan::Project { input, columns }, visible)
}

fn build_limit_offset(
    plan: LogicalPlan,
    limit: &Option<ast::Expr>,
    offset: &Option<ast::Offset>,
) -> Result<LogicalPlan> {
    let plan = match offset {
        Some(ast::Offset { value, .. }) => {
            let count = ast_expr_to_usize(value)?;
            if count > 0 {
                LogicalPlan::Offset {
                    input: Box::new(plan),
                    count,
                }
            } else {
                plan
            }
        }
        None => plan,
    };
    match limit {
        None => Ok(plan),
        Some(expr) => {
            let count = ast_expr_to_usize(expr)?;
            Ok(LogicalPlan::Limit {
                input: Box::new(plan),
                count,
            })
        }
    }
}

fn ast_expr_to_usize(expr: &ast::Expr) -> Result<usize> {
    match expr {
        ast::Expr::Value(v) => match v {
            ast::Value::Number(n, _) => n
                .parse::<usize>()
                .map_err(|_| sql_err(format!("invalid limit: {n}"))),
            _ => Err(sql_err("LIMIT must be a number")),
        },
        _ => Err(sql_err("LIMIT must be a literal number")),
    }
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` string (proleptic Gregorian).
fn parse_date_days(value: &str) -> Option<i64> {
    kaveon_core::predicate::date_literal_days(value)
}

fn ast_expr_to_expr(expr: &ast::Expr) -> Result<Expr> {
    match expr {
        ast::Expr::Identifier(ident) => Ok(Expr::Column(ident.value.clone())),
        ast::Expr::CompoundIdentifier(parts) => {
            let name = parts
                .iter()
                .map(|p| p.value.as_str())
                .collect::<Vec<_>>()
                .join(".");
            Ok(Expr::Column(name))
        }
        ast::Expr::Value(v) => ast_value_to_expr(v),
        ast::Expr::BinaryOp { left, op, right } => {
            if let Some(expr) = interval_arithmetic(left, op, right)? {
                return Ok(expr);
            }
            let l = ast_expr_to_expr(left)?;
            let r = ast_expr_to_expr(right)?;
            match ast_binop_to_binop(op) {
                Some(binop) => Ok(Expr::BinaryOp {
                    left: Box::new(l),
                    op: binop,
                    right: Box::new(r),
                }),
                None => match op {
                    ast::BinaryOperator::And => Ok(Expr::And(Box::new(l), Box::new(r))),
                    ast::BinaryOperator::Or => Ok(Expr::Or(Box::new(l), Box::new(r))),
                    _ => Err(sql_err(format!("unsupported operator: {op}"))),
                },
            }
        }
        ast::Expr::UnaryOp {
            op: ast::UnaryOperator::Not,
            expr,
        } => {
            let inner = ast_expr_to_expr(expr)?;
            Ok(Expr::Not(Box::new(inner)))
        }
        ast::Expr::UnaryOp {
            op: ast::UnaryOperator::Minus,
            expr,
        } => {
            let inner = ast_expr_to_expr(expr)?;
            Ok(Expr::BinaryOp {
                left: Box::new(Expr::Literal(ScalarValue::Int64(0))),
                op: BinaryOp::Minus,
                right: Box::new(inner),
            })
        }
        ast::Expr::IsNull(expr) => Ok(Expr::IsNull(Box::new(ast_expr_to_expr(expr)?))),
        ast::Expr::IsNotNull(expr) => Ok(Expr::IsNotNull(Box::new(ast_expr_to_expr(expr)?))),
        ast::Expr::Nested(inner) => ast_expr_to_expr(inner),
        ast::Expr::Function(func) => ast_function_to_expr(func),
        ast::Expr::Wildcard(_) => Ok(Expr::Star),

        // ── CASE ────────────────────────────────────────────────────────
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            let operand = operand
                .as_ref()
                .map(|e| ast_expr_to_expr(e).map(Box::new))
                .transpose()?;
            let when_then: Vec<(Expr, Expr)> = conditions
                .iter()
                .zip(results.iter())
                .map(|(c, r)| Ok((ast_expr_to_expr(c)?, ast_expr_to_expr(r)?)))
                .collect::<Result<_>>()?;
            let else_expr = else_result
                .as_ref()
                .map(|e| ast_expr_to_expr(e).map(Box::new))
                .transpose()?;
            Ok(Expr::Case {
                operand,
                when_then,
                else_expr,
            })
        }

        // ── LIKE / ILIKE ────────────────────────────────────────────────
        ast::Expr::Like {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(ast_expr_to_expr(expr)?),
            pattern: Box::new(ast_expr_to_expr(pattern)?),
            negated: *negated,
            case_insensitive: false,
        }),
        ast::Expr::ILike {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(ast_expr_to_expr(expr)?),
            pattern: Box::new(ast_expr_to_expr(pattern)?),
            negated: *negated,
            case_insensitive: true,
        }),

        // ── BETWEEN ─────────────────────────────────────────────────────
        ast::Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(ast_expr_to_expr(expr)?),
            low: Box::new(ast_expr_to_expr(low)?),
            high: Box::new(ast_expr_to_expr(high)?),
            negated: *negated,
        }),

        // ── IN (list) ───────────────────────────────────────────────────
        ast::Expr::InList {
            expr,
            list,
            negated,
        } => {
            let e = ast_expr_to_expr(expr)?;
            let items: Vec<Expr> = list.iter().map(ast_expr_to_expr).collect::<Result<_>>()?;
            Ok(Expr::InList {
                expr: Box::new(e),
                list: items,
                negated: *negated,
            })
        }

        // A DATE literal is its day number: the Engine's date columns are
        // days since the epoch, so `EventDate >= DATE '2013-07-01'` compares
        // integers with integers.
        ast::Expr::TypedString {
            data_type: ast::DataType::Date,
            value,
        } => {
            let days = parse_date_days(value)
                .ok_or_else(|| sql_err(format!("invalid DATE literal: '{value}'")))?;
            Ok(Expr::Literal(ScalarValue::Int64(days)))
        }

        // ── CAST ────────────────────────────────────────────────────────
        ast::Expr::Cast {
            expr, data_type, ..
        } => {
            let inner = ast_expr_to_expr(expr)?;
            let target = ast_data_type_to_cast_target(data_type)?;
            Ok(Expr::Cast {
                expr: Box::new(inner),
                data_type: target,
            })
        }

        // ── EXTRACT ──────────────────────────────────────────────────────
        ast::Expr::Extract { field, expr, .. } => {
            let date_field = ast_date_field_to_date_field(field)?;
            Ok(Expr::Extract {
                field: date_field,
                expr: Box::new(ast_expr_to_expr(expr)?),
            })
        }

        // ── Subqueries ──────────────────────────────────────────────────
        // IN/EXISTS subqueries are handled at plan level by build_where()
        ast::Expr::InSubquery { .. } => Err(sql_err(
            "IN subquery in this position is not supported; use it in WHERE",
        )),
        ast::Expr::Exists { .. } => Err(sql_err(
            "EXISTS in this position is not supported; use it in WHERE",
        )),
        ast::Expr::Subquery(_) => Err(sql_err("scalar subqueries are not yet supported")),

        ast::Expr::Interval(_) => Err(sql_err(
            "an INTERVAL is only supported added to or subtracted from a date",
        )),

        _ => Err(sql_err(format!("unsupported expression: {expr}"))),
    }
}

/// `date + INTERVAL 'n' DAY` and `date - INTERVAL 'n' DAY` lower to
/// arithmetic on the day number, so any date expression takes a day
/// interval. Month and year intervals shift the calendar, which the Engine
/// evaluates at lowering time against a DATE literal (`DATE '1993-07-01' +
/// INTERVAL '3' MONTH` is the day number of 1993-10-01); a day past the end
/// of the target month clamps to that month's last day, as Trino does.
/// Returns None when neither operand is an interval.
fn interval_arithmetic(
    left: &ast::Expr,
    op: &ast::BinaryOperator,
    right: &ast::Expr,
) -> Result<Option<Expr>> {
    let (date, interval, negate) = match (left, op, right) {
        (_, ast::BinaryOperator::Plus, ast::Expr::Interval(interval)) => (left, interval, false),
        (_, ast::BinaryOperator::Minus, ast::Expr::Interval(interval)) => (left, interval, true),
        (ast::Expr::Interval(interval), ast::BinaryOperator::Plus, _) => (right, interval, false),
        (ast::Expr::Interval(_), _, _) | (_, _, ast::Expr::Interval(_)) => {
            return Err(sql_err(format!(
                "unsupported INTERVAL arithmetic: {left} {op} {right}"
            )));
        }
        _ => return Ok(None),
    };
    let text = match interval.value.as_ref() {
        ast::Expr::Value(ast::Value::SingleQuotedString(text)) => text.trim(),
        ast::Expr::Value(ast::Value::Number(text, _)) => text.as_str(),
        other => return Err(sql_err(format!("unsupported INTERVAL value: {other}"))),
    };
    let count: i64 = text
        .parse()
        .map_err(|_| sql_err(format!("invalid INTERVAL value: '{text}'")))?;
    let count = if negate { -count } else { count };
    if interval.last_field.is_some() {
        return Err(sql_err(format!(
            "unsupported INTERVAL field range: {interval}"
        )));
    }
    let date = ast_expr_to_expr(date)?;
    match &interval.leading_field {
        Some(ast::DateTimeField::Day) | None => Ok(Some(Expr::BinaryOp {
            left: Box::new(date),
            op: BinaryOp::Plus,
            right: Box::new(Expr::Literal(ScalarValue::Int64(count))),
        })),
        Some(ast::DateTimeField::Month) | Some(ast::DateTimeField::Year) => {
            let months = if matches!(interval.leading_field, Some(ast::DateTimeField::Year)) {
                count * 12
            } else {
                count
            };
            let Expr::Literal(ScalarValue::Int64(days)) = date else {
                return Err(sql_err(format!(
                    "INTERVAL {} arithmetic needs a DATE literal operand",
                    interval.leading_field.as_ref().expect("month or year")
                )));
            };
            Ok(Some(Expr::Literal(ScalarValue::Int64(add_months(
                days, months,
            )))))
        }
        Some(other) => Err(sql_err(format!("unsupported INTERVAL field: {other}"))),
    }
}

/// The day number `months` calendar months after the day number `days`;
/// a day past the end of the target month clamps to that month's last day.
fn add_months(days: i64, months: i64) -> i64 {
    let (year, month, day) = civil_from_days(days);
    let total = year * 12 + (month - 1) + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    let day = day.min(days_in_month(year, month));
    parse_date_days(&format!("{year:04}-{month:02}-{day:02}"))
        .expect("a calendar day within its month is a valid date")
}

/// (year, month, day) for a day number since 1970-01-01, proleptic
/// Gregorian; the inverse of `date_literal_days`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

fn ast_data_type_to_cast_target(dt: &ast::DataType) -> Result<CastTarget> {
    match dt {
        ast::DataType::Boolean => Ok(CastTarget::Boolean),
        ast::DataType::Int(_) | ast::DataType::Integer(_) | ast::DataType::Int4(_) => {
            Ok(CastTarget::Int32)
        }
        ast::DataType::BigInt(_) | ast::DataType::Int8(_) => Ok(CastTarget::Int64),
        ast::DataType::Float(_)
        | ast::DataType::Double
        | ast::DataType::DoublePrecision
        | ast::DataType::Real
        | ast::DataType::Float8 => Ok(CastTarget::Float64),
        ast::DataType::Varchar(_)
        | ast::DataType::Text
        | ast::DataType::String(_)
        | ast::DataType::Char(_)
        | ast::DataType::CharVarying(_) => Ok(CastTarget::Utf8),
        ast::DataType::Decimal(info) | ast::DataType::Numeric(info) => {
            let (p, s) = match info {
                ast::ExactNumberInfo::PrecisionAndScale(p, s) => (*p as u8, *s as i8),
                ast::ExactNumberInfo::Precision(p) => (*p as u8, 0),
                ast::ExactNumberInfo::None => (38, 10),
            };
            Ok(CastTarget::Decimal128 {
                precision: p,
                scale: s,
            })
        }
        other => Err(sql_err(format!("unsupported CAST target type: {other}"))),
    }
}

fn ast_value_to_expr(value: &ast::Value) -> Result<Expr> {
    match value {
        ast::Value::Number(n, _) => {
            if let Ok(i) = n.parse::<i64>() {
                Ok(Expr::Literal(ScalarValue::Int64(i)))
            } else if n.contains('.') {
                let (int_part, frac_part) = n.split_once('.').unwrap();
                let scale = frac_part.len() as i8;
                let combined = format!("{int_part}{frac_part}");
                if let Ok(v) = combined.parse::<i128>() {
                    let precision = n.replace(['-', '.'], "").len() as u8;
                    let precision = precision.max(scale as u8 + 1);
                    Ok(Expr::Literal(ScalarValue::Decimal128 {
                        value: v,
                        precision: precision.min(38),
                        scale,
                    }))
                } else if let Ok(f) = n.parse::<f64>() {
                    Ok(Expr::Literal(ScalarValue::Float64(f)))
                } else {
                    Err(sql_err(format!("invalid number: {n}")))
                }
            } else if let Ok(f) = n.parse::<f64>() {
                Ok(Expr::Literal(ScalarValue::Float64(f)))
            } else {
                Err(sql_err(format!("invalid number: {n}")))
            }
        }
        ast::Value::SingleQuotedString(s) | ast::Value::DoubleQuotedString(s) => {
            Ok(Expr::Literal(ScalarValue::Utf8(s.clone())))
        }
        ast::Value::Boolean(b) => Ok(Expr::Literal(ScalarValue::Bool(*b))),
        ast::Value::Null => Ok(Expr::Literal(ScalarValue::Null)),
        _ => Err(sql_err(format!("unsupported value: {value}"))),
    }
}

fn ast_binop_to_binop(op: &ast::BinaryOperator) -> Option<BinaryOp> {
    match op {
        ast::BinaryOperator::Eq => Some(BinaryOp::Eq),
        ast::BinaryOperator::NotEq => Some(BinaryOp::Ne),
        ast::BinaryOperator::Lt => Some(BinaryOp::Lt),
        ast::BinaryOperator::LtEq => Some(BinaryOp::Le),
        ast::BinaryOperator::Gt => Some(BinaryOp::Gt),
        ast::BinaryOperator::GtEq => Some(BinaryOp::Ge),
        ast::BinaryOperator::Plus => Some(BinaryOp::Plus),
        ast::BinaryOperator::Minus => Some(BinaryOp::Minus),
        ast::BinaryOperator::Multiply => Some(BinaryOp::Multiply),
        ast::BinaryOperator::Divide => Some(BinaryOp::Divide),
        ast::BinaryOperator::Modulo => Some(BinaryOp::Modulo),
        ast::BinaryOperator::StringConcat => Some(BinaryOp::StringConcat),
        _ => None,
    }
}

fn ast_function_to_expr(func: &ast::Function) -> Result<Expr> {
    let name = func.name.to_string().to_uppercase();
    let args: Vec<Expr> = match &func.args {
        ast::FunctionArguments::List(arg_list) => arg_list
            .args
            .iter()
            .map(|arg| match arg {
                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(e)) => ast_expr_to_expr(e),
                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Wildcard) => Ok(Expr::Star),
                _ => Err(sql_err(format!("unsupported function argument: {arg}"))),
            })
            .collect::<Result<_>>()?,
        ast::FunctionArguments::None => Vec::new(),
        _ => return Err(sql_err(format!("unsupported function arguments in {name}"))),
    };

    if let Some(over) = &func.over {
        if func.null_treatment.is_some() || func.filter.is_some() || !func.within_group.is_empty() {
            return Err(sql_err(
                "window NULL treatment, FILTER, and WITHIN GROUP are unsupported",
            ));
        }
        if let ast::FunctionArguments::List(list) = &func.args
            && (matches!(
                list.duplicate_treatment,
                Some(ast::DuplicateTreatment::Distinct)
            ) || !list.clauses.is_empty())
        {
            return Err(sql_err(
                "DISTINCT and argument clauses in window functions are unsupported",
            ));
        }
        let (partition_by, order_by, frame) = match over {
            ast::WindowType::WindowSpec(spec) => {
                let partition_by = spec
                    .partition_by
                    .iter()
                    .map(ast_expr_to_expr)
                    .collect::<Result<Vec<_>>>()?;
                let order_by = spec
                    .order_by
                    .iter()
                    .map(|ob| {
                        if ob.nulls_first == Some(true) {
                            return Err(sql_err("window NULLS FIRST ordering is unsupported"));
                        }
                        let expr = ast_expr_to_expr(&ob.expr)?;
                        let asc = ob.asc.unwrap_or(true);
                        Ok((expr, asc))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let frame = spec
                    .window_frame
                    .as_ref()
                    .map(ast_window_frame)
                    .transpose()?;
                (partition_by, order_by, frame)
            }
            ast::WindowType::NamedWindow(_) => {
                return Err(sql_err("named windows are not supported"));
            }
        };
        return Ok(Expr::WindowFunction {
            name,
            args,
            partition_by,
            order_by,
            frame,
        });
    }

    Ok(Expr::Function { name, args })
}

fn contains_aggregate_select_item(item: &ast::SelectItem) -> bool {
    match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
            contains_aggregate_expr(expr)
        }
        _ => false,
    }
}

fn contains_aggregate_expr(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Function(func) => {
            if func.over.is_some() {
                return false;
            }
            let name = func.name.to_string().to_uppercase();
            matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            contains_aggregate_expr(left) || contains_aggregate_expr(right)
        }
        ast::Expr::UnaryOp { expr, .. } => contains_aggregate_expr(expr),
        ast::Expr::Nested(inner) => contains_aggregate_expr(inner),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            operand.as_ref().is_some_and(|e| contains_aggregate_expr(e))
                || conditions.iter().any(contains_aggregate_expr)
                || results.iter().any(contains_aggregate_expr)
                || else_result
                    .as_ref()
                    .is_some_and(|e| contains_aggregate_expr(e))
        }
        ast::Expr::Cast { expr, .. } => contains_aggregate_expr(expr),
        _ => false,
    }
}

fn collect_aggregates_from_select_item(
    item: &ast::SelectItem,
    out: &mut Vec<AggregateExpr>,
) -> Result<()> {
    match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
            collect_aggregates_from_ast_expr(expr, out)
        }
        _ => Ok(()),
    }
}

fn collect_aggregates_from_ast_expr(expr: &ast::Expr, out: &mut Vec<AggregateExpr>) -> Result<()> {
    match expr {
        ast::Expr::Function(func) => {
            if func.over.is_some() {
                return Ok(());
            }
            let name = func.name.to_string().to_uppercase();
            if matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX") {
                let arg = match &func.args {
                    ast::FunctionArguments::List(args) => {
                        if args.args.is_empty() {
                            Expr::Star
                        } else {
                            match &args.args[0] {
                                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Wildcard) => {
                                    Expr::Star
                                }
                                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(e)) => {
                                    ast_expr_to_expr(e)?
                                }
                                _ => return Err(sql_err("unsupported aggregate argument")),
                            }
                        }
                    }
                    _ => Expr::Star,
                };
                let distinct = matches!(
                    &func.args,
                    ast::FunctionArguments::List(args)
                        if args.duplicate_treatment == Some(ast::DuplicateTreatment::Distinct)
                );
                if distinct && matches!(arg, Expr::Star) {
                    return Err(sql_err(format!("{name}(DISTINCT *) is not supported")));
                }
                if distinct && matches!(name.as_str(), "MIN" | "MAX") {
                    return Err(sql_err(format!("DISTINCT is not supported for {name}")));
                }
                let agg = match name.as_str() {
                    "COUNT" => AggregateExpr::Count {
                        expr: arg,
                        distinct,
                    },
                    "SUM" => AggregateExpr::Sum {
                        expr: arg,
                        distinct,
                    },
                    "AVG" => AggregateExpr::Avg {
                        expr: arg,
                        distinct,
                    },
                    "MIN" => AggregateExpr::Min(arg),
                    "MAX" => AggregateExpr::Max(arg),
                    _ => unreachable!(),
                };
                out.push(agg);
            }
            Ok(())
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            collect_aggregates_from_ast_expr(left, out)?;
            collect_aggregates_from_ast_expr(right, out)
        }
        ast::Expr::UnaryOp { expr, .. } => collect_aggregates_from_ast_expr(expr, out),
        ast::Expr::Nested(inner) => collect_aggregates_from_ast_expr(inner, out),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(op) = operand {
                collect_aggregates_from_ast_expr(op, out)?;
            }
            for c in conditions {
                collect_aggregates_from_ast_expr(c, out)?;
            }
            for r in results {
                collect_aggregates_from_ast_expr(r, out)?;
            }
            if let Some(e) = else_result {
                collect_aggregates_from_ast_expr(e, out)?;
            }
            Ok(())
        }
        ast::Expr::Cast { expr, .. } => collect_aggregates_from_ast_expr(expr, out),
        _ => Ok(()),
    }
}

fn collect_window_exprs(select: &ast::Select) -> Result<Vec<Expr>> {
    let mut window_exprs = Vec::new();
    for item in &select.projection {
        match item {
            ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
                collect_windows_from_ast_expr(expr, &mut window_exprs)?;
            }
            _ => {}
        }
    }
    Ok(window_exprs)
}

fn collect_windows_from_ast_expr(expr: &ast::Expr, out: &mut Vec<Expr>) -> Result<()> {
    match expr {
        ast::Expr::Function(func) if func.over.is_some() => {
            out.push(ast_function_to_expr(func)?);
            Ok(())
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            collect_windows_from_ast_expr(left, out)?;
            collect_windows_from_ast_expr(right, out)
        }
        ast::Expr::Nested(inner)
        | ast::Expr::Cast { expr: inner, .. }
        | ast::Expr::UnaryOp { expr: inner, .. } => collect_windows_from_ast_expr(inner, out),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(op) = operand {
                collect_windows_from_ast_expr(op, out)?;
            }
            for c in conditions {
                collect_windows_from_ast_expr(c, out)?;
            }
            for r in results {
                collect_windows_from_ast_expr(r, out)?;
            }
            if let Some(e) = else_result {
                collect_windows_from_ast_expr(e, out)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ast_date_field_to_date_field(field: &ast::DateTimeField) -> Result<DateField> {
    match field {
        ast::DateTimeField::Year => Ok(DateField::Year),
        ast::DateTimeField::Month => Ok(DateField::Month),
        ast::DateTimeField::Day => Ok(DateField::Day),
        ast::DateTimeField::Hour => Ok(DateField::Hour),
        ast::DateTimeField::Minute => Ok(DateField::Minute),
        ast::DateTimeField::Second => Ok(DateField::Second),
        ast::DateTimeField::Dow => Ok(DateField::DayOfWeek),
        ast::DateTimeField::Doy => Ok(DateField::DayOfYear),
        ast::DateTimeField::Quarter => Ok(DateField::Quarter),
        ast::DateTimeField::Week(_) => Ok(DateField::Week),
        ast::DateTimeField::Epoch => Ok(DateField::Epoch),
        other => Err(sql_err(format!("unsupported EXTRACT field: {other}"))),
    }
}

fn ast_window_frame(frame: &ast::WindowFrame) -> Result<WindowFrame> {
    let units = match frame.units {
        ast::WindowFrameUnits::Rows => WindowFrameUnits::Rows,
        ast::WindowFrameUnits::Range => WindowFrameUnits::Range,
        ast::WindowFrameUnits::Groups => WindowFrameUnits::Groups,
    };
    let start = ast_window_frame_bound(&frame.start_bound)?;
    let end = frame
        .end_bound
        .as_ref()
        .map(ast_window_frame_bound)
        .transpose()?
        .unwrap_or(WindowFrameBound::CurrentRow);
    Ok(WindowFrame { units, start, end })
}

fn ast_window_frame_bound(bound: &ast::WindowFrameBound) -> Result<WindowFrameBound> {
    match bound {
        ast::WindowFrameBound::CurrentRow => Ok(WindowFrameBound::CurrentRow),
        ast::WindowFrameBound::Preceding(None) => Ok(WindowFrameBound::UnboundedPreceding),
        ast::WindowFrameBound::Preceding(Some(expr)) => match expr.as_ref() {
            ast::Expr::Value(v) => match v {
                ast::Value::Number(n, _) => n
                    .parse::<u64>()
                    .map(WindowFrameBound::Preceding)
                    .map_err(|_| sql_err(format!("invalid frame offset: {n}"))),
                _ => Err(sql_err("frame offset must be a number")),
            },
            _ => Err(sql_err("frame offset must be a literal")),
        },
        ast::WindowFrameBound::Following(None) => Ok(WindowFrameBound::UnboundedFollowing),
        ast::WindowFrameBound::Following(Some(expr)) => match expr.as_ref() {
            ast::Expr::Value(v) => match v {
                ast::Value::Number(n, _) => n
                    .parse::<u64>()
                    .map(WindowFrameBound::Following)
                    .map_err(|_| sql_err(format!("invalid frame offset: {n}"))),
                _ => Err(sql_err("frame offset must be a number")),
            },
            _ => Err(sql_err("frame offset must be a literal")),
        },
    }
}

fn validate_uncorrelated(plan: &LogicalPlan) -> Result<()> {
    fn visit<'a>(
        plan: &'a LogicalPlan,
        relations: &mut Vec<String>,
        expressions: &mut Vec<&'a Expr>,
    ) {
        match plan {
            LogicalPlan::Scan { table, alias, .. } => {
                relations.push(alias.clone().unwrap_or_else(|| table.clone()));
                if alias.is_none() {
                    relations.push(table.rsplit('.').next().unwrap().to_owned());
                }
            }
            LogicalPlan::Filter { input, predicate } => {
                expressions.push(predicate);
                visit(input, relations, expressions);
            }
            LogicalPlan::Project { input, columns } => {
                expressions.extend(columns);
                visit(input, relations, expressions);
            }
            LogicalPlan::Sort { input, order_by } => {
                expressions.extend(order_by.iter().map(|(e, _)| e));
                visit(input, relations, expressions);
            }
            LogicalPlan::Window {
                input,
                window_exprs,
            } => {
                expressions.extend(window_exprs);
                visit(input, relations, expressions);
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                expressions.extend(group_by);
                expressions.extend(aggregates.iter().map(|a| match a {
                    AggregateExpr::Count { expr, .. }
                    | AggregateExpr::Sum { expr, .. }
                    | AggregateExpr::Avg { expr, .. }
                    | AggregateExpr::Min(expr)
                    | AggregateExpr::Max(expr) => expr,
                }));
                visit(input, relations, expressions);
            }
            LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input } => visit(input, relations, expressions),
            LogicalPlan::Union { inputs, .. } => {
                for input in inputs {
                    visit(input, relations, expressions);
                }
            }
            LogicalPlan::Join {
                left,
                right,
                condition,
                ..
            } => {
                expressions.extend(condition);
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
            }
            | LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
            } => {
                expressions.extend([left_key, right_key]);
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
            LogicalPlan::Intersect { left, right } | LogicalPlan::Except { left, right } => {
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
        }
    }
    fn check(expr: &Expr, relations: &[String]) -> Result<()> {
        match expr {
            Expr::Column(name) => {
                if let Some((qualifier, _)) = name.rsplit_once('.')
                    && !relations.iter().any(|r| r.eq_ignore_ascii_case(qualifier))
                {
                    return Err(sql_err(format!(
                        "correlated subqueries are unsupported: {name}"
                    )));
                }
            }
            Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
                check(left, relations)?;
                check(right, relations)?;
            }
            Expr::IsNull(e)
            | Expr::IsNotNull(e)
            | Expr::Not(e)
            | Expr::Alias { expr: e, .. }
            | Expr::Cast { expr: e, .. }
            | Expr::Extract { expr: e, .. } => check(e, relations)?,
            Expr::Function { args, .. } => {
                for e in args {
                    check(e, relations)?;
                }
            }
            Expr::WindowFunction {
                args,
                partition_by,
                order_by,
                ..
            } => {
                for e in args
                    .iter()
                    .chain(partition_by)
                    .chain(order_by.iter().map(|(e, _)| e))
                {
                    check(e, relations)?;
                }
            }
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => {
                for e in operand.iter().chain(else_expr) {
                    check(e, relations)?;
                }
                for (a, b) in when_then {
                    check(a, relations)?;
                    check(b, relations)?;
                }
            }
            Expr::Like { expr, pattern, .. } => {
                check(expr, relations)?;
                check(pattern, relations)?;
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                check(expr, relations)?;
                check(low, relations)?;
                check(high, relations)?;
            }
            Expr::InList { expr, list, .. } => {
                check(expr, relations)?;
                for e in list {
                    check(e, relations)?;
                }
            }
            Expr::Literal(_) | Expr::Star => {}
        }
        Ok(())
    }
    let mut relations = Vec::new();
    let mut expressions = Vec::new();
    visit(plan, &mut relations, &mut expressions);
    for expr in expressions {
        check(expr, &relations)?;
    }
    Ok(())
}

fn sql_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Sql(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_defaults_to_distinct_and_unsupported_multisets_fail() {
        assert!(matches!(
            sql_to_logical_plan("SELECT x FROM a UNION SELECT x FROM b").unwrap(),
            LogicalPlan::Distinct { .. }
        ));
        assert!(sql_to_logical_plan("SELECT x FROM a INTERSECT ALL SELECT x FROM b").is_err());
        assert!(sql_to_logical_plan("SELECT x FROM a EXCEPT ALL SELECT x FROM b").is_err());
    }

    #[test]
    fn order_by_a_repeated_select_item_orders_by_its_output() {
        let plan =
            sql_to_logical_plan("SELECT k, COUNT(*) FROM t GROUP BY k ORDER BY COUNT(*) DESC")
                .unwrap();
        let LogicalPlan::Sort { input, order_by } = plan else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("expr_1".into()), false)]);
        let LogicalPlan::Project { columns, .. } = *input else {
            panic!("projection");
        };
        assert!(matches!(&columns[1], Expr::Alias { name, .. } if name == "expr_1"));
        // An aliased item keeps its alias; a column key is untouched.
        let plan =
            sql_to_logical_plan("SELECT k, SUM(v) AS total FROM t GROUP BY k ORDER BY SUM(v), k")
                .unwrap();
        let LogicalPlan::Sort { order_by, .. } = plan else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("total".into()), true),
                (Expr::Column("k".into()), true)
            ]
        );
    }

    #[test]
    fn order_by_a_lowered_group_expression_resolves_to_its_column() {
        let plan = sql_to_logical_plan(
            "SELECT t - t % 60 AS m, COUNT(*) AS n FROM h GROUP BY t - t % 60 ORDER BY t - t % 60 LIMIT 10",
        )
        .unwrap();
        // The lowered expression is selected as `m`: the sort orders by
        // that output column and nothing rides along.
        let LogicalPlan::Limit { input, .. } = plan else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, input } = *input else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("m".into()), true)]);
        assert!(matches!(*input, LogicalPlan::Project { .. }));
    }

    #[test]
    fn order_by_a_qualified_column_selected_under_an_alias_uses_the_alias() {
        // `u.country AS user_country` is what `ORDER BY u.country` means;
        // `t.country` does not stand in for it just because both are
        // "country".
        let plan = sql_to_logical_plan(
            "SELECT t.country, u.country AS user_country, COUNT(*) AS n FROM t JOIN u ON u.id = t.id GROUP BY t.country, u.country ORDER BY n DESC, t.country, u.country LIMIT 3",
        )
        .unwrap();
        let LogicalPlan::Limit { input, .. } = plan else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, .. } = *input else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("n".into()), false),
                (Expr::Column("t.country".into()), true),
                (Expr::Column("user_country".into()), true),
            ]
        );
        // An unselected qualified column still rides along, even when a
        // column of the same bare name is selected.
        let plan = sql_to_logical_plan(
            "SELECT t.country, COUNT(*) AS n FROM t JOIN u ON u.id = t.id GROUP BY t.country, u.country ORDER BY u.country LIMIT 3",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("visible projection above the ride-along");
        };
        assert_eq!(
            columns,
            vec![Expr::Column("t.country".into()), Expr::Column("n".into())]
        );
        let LogicalPlan::Limit { input, .. } = *input else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, .. } = *input else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("u.country".into()), true)]);
    }

    #[test]
    fn order_by_an_unselected_column_rides_through_and_is_dropped() {
        let plan = sql_to_logical_plan(
            "SELECT phrase FROM t WHERE phrase <> '' ORDER BY event_time, phrase LIMIT 10",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("visible projection on top");
        };
        assert_eq!(columns, vec![Expr::Column("phrase".into())]);
        let LogicalPlan::Limit { input, count: 10 } = *input else {
            panic!("limit under the visible projection");
        };
        let LogicalPlan::Sort { input, order_by } = *input else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("event_time".into()), true),
                (Expr::Column("phrase".into()), true)
            ]
        );
        let LogicalPlan::Project { columns, .. } = *input else {
            panic!("extended projection");
        };
        assert_eq!(
            columns,
            vec![
                Expr::Column("phrase".into()),
                Expr::Column("event_time".into())
            ]
        );
        // A selected key changes nothing.
        let plan = sql_to_logical_plan("SELECT a, b FROM t ORDER BY b").unwrap();
        assert!(matches!(plan, LogicalPlan::Sort { .. }));
    }

    #[test]
    fn date_literals_are_day_numbers() {
        assert_eq!(parse_date_days("1970-01-01"), Some(0));
        assert_eq!(parse_date_days("2013-07-01"), Some(15887));
        assert_eq!(parse_date_days("1969-12-31"), Some(-1));
        assert_eq!(parse_date_days("2013-13-01"), None);
        let plan = sql_to_logical_plan("SELECT x FROM t WHERE d >= DATE '2013-07-01'").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("filter");
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(Expr::Column("d".into())),
                op: BinaryOp::Ge,
                right: Box::new(Expr::Literal(ScalarValue::Int64(15887))),
            }
        );
    }

    #[test]
    fn interval_arithmetic_on_dates_lowers_to_day_numbers() {
        // A day interval is day-number arithmetic, on a literal or a column.
        let filter = |sql: &str| {
            let plan = sql_to_logical_plan(sql).unwrap();
            let LogicalPlan::Project { input, .. } = plan else {
                panic!("projection");
            };
            let LogicalPlan::Filter { predicate, .. } = *input else {
                panic!("filter");
            };
            predicate
        };
        let day = |days: i64| Box::new(Expr::Literal(ScalarValue::Int64(days)));
        assert_eq!(
            filter("SELECT x FROM t WHERE d <= DATE '1998-12-01' - INTERVAL '90' DAY"),
            Expr::BinaryOp {
                left: Box::new(Expr::Column("d".into())),
                op: BinaryOp::Le,
                right: Box::new(Expr::BinaryOp {
                    left: day(parse_date_days("1998-12-01").unwrap()),
                    op: BinaryOp::Plus,
                    right: day(-90),
                }),
            }
        );
        assert_eq!(
            filter("SELECT x FROM t WHERE d + INTERVAL '7' DAY < e"),
            Expr::BinaryOp {
                left: Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("d".into())),
                    op: BinaryOp::Plus,
                    right: day(7),
                }),
                op: BinaryOp::Lt,
                right: Box::new(Expr::Column("e".into())),
            }
        );
        // Month and year intervals shift the calendar of a DATE literal.
        let shifted = |sql: &str, expected: &str| {
            assert_eq!(
                filter(sql),
                Expr::BinaryOp {
                    left: Box::new(Expr::Column("d".into())),
                    op: BinaryOp::Lt,
                    right: day(parse_date_days(expected).unwrap()),
                },
                "{sql}"
            );
        };
        shifted(
            "SELECT x FROM t WHERE d < DATE '1993-07-01' + INTERVAL '3' MONTH",
            "1993-10-01",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1994-01-01' + INTERVAL '1' YEAR",
            "1995-01-01",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1995-11-15' + INTERVAL '3' MONTH",
            "1996-02-15",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1998-01-31' + INTERVAL '1' MONTH",
            "1998-02-28",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1996-01-31' + INTERVAL '1' MONTH",
            "1996-02-29",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1996-03-31' - INTERVAL '1' MONTH",
            "1996-02-29",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '2000-02-29' - INTERVAL '1' YEAR",
            "1999-02-28",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1993-01-01' - INTERVAL '13' MONTH",
            "1991-12-01",
        );
        // A calendar shift of a column has no day-number form.
        let error = sql_to_logical_plan("SELECT x FROM t WHERE d + INTERVAL '1' MONTH < e")
            .unwrap_err()
            .to_string();
        assert!(error.contains("needs a DATE literal operand"), "{error}");
        assert!(sql_to_logical_plan("SELECT INTERVAL '1' DAY FROM t").is_err());
        assert!(
            sql_to_logical_plan("SELECT x FROM t WHERE d < DATE '1993-07-01' + INTERVAL '1' HOUR")
                .is_err()
        );
    }

    #[test]
    fn civil_dates_round_trip_through_day_numbers() {
        for date in [
            "1970-01-01",
            "1969-12-31",
            "1992-01-01",
            "1996-02-29",
            "1998-12-31",
            "2000-02-29",
            "2026-09-16",
            "1900-03-01",
            "2100-02-28",
        ] {
            let days = parse_date_days(date).unwrap();
            let (year, month, day) = civil_from_days(days);
            assert_eq!(format!("{year:04}-{month:02}-{day:02}"), date);
        }
    }

    #[test]
    fn group_by_without_aggregates_is_distinct_over_the_keys() {
        let plan =
            sql_to_logical_plan("SELECT country FROM u WHERE locale = 1 GROUP BY country").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("outer projection");
        };
        let LogicalPlan::Distinct { input } = *input else {
            panic!("distinct over the keys");
        };
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("key projection");
        };
        assert_eq!(columns, vec![Expr::Column("country".into())]);
        assert!(matches!(*input, LogicalPlan::Filter { .. }));
        // An expression key still aggregates.
        assert!(matches!(
            sql_to_logical_plan("SELECT UPPER(country) FROM u GROUP BY UPPER(country)").unwrap(),
            LogicalPlan::Project { input, .. } if matches!(*input, LogicalPlan::Aggregate { .. })
        ));
    }

    #[test]
    fn lowers_group_and_aggregate_expressions_below_aggregation() {
        let plan = sql_to_logical_plan("SELECT CAST(CAST(x AS DECIMAL(20,4)) AS VARCHAR) AS k, SUM(CAST(x AS DECIMAL(20,4))) FROM t GROUP BY CAST(x AS DECIMAL(20,4))").unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("project");
        };
        let LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } = *input
        else {
            panic!("aggregate");
        };
        assert!(matches!(&*input, LogicalPlan::Project { .. }));
        assert!(matches!(&group_by[0], Expr::Column(name) if name == "__kaveon_group_0"));
        assert!(
            matches!(&aggregates[0], AggregateExpr::Sum { expr: Expr::Column(name), .. } if name == "__kaveon_arg_0")
        );
        assert!(matches!(&columns[1], Expr::Column(name) if name == "sum___kaveon_arg_0"));
    }

    #[test]
    fn an_expression_over_a_simple_aggregate_is_lowered_to_its_column() {
        let plan = sql_to_logical_plan("SELECT 0.2 * avg(x) AS a, max(y) FROM t").unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("project");
        };
        assert_eq!(
            columns[0],
            Expr::Alias {
                expr: Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Literal(ScalarValue::Decimal128 {
                        value: 2,
                        precision: 2,
                        scale: 1
                    })),
                    op: BinaryOp::Multiply,
                    right: Box::new(Expr::Column("avg___kaveon_arg_0".into())),
                }),
                name: "a".into()
            }
        );
        assert_eq!(columns[1], Expr::Column("max___kaveon_arg_1".into()));
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("aggregate");
        };
        assert!(
            matches!(&aggregates[0], AggregateExpr::Avg { expr: Expr::Column(name), .. } if name == "__kaveon_arg_0")
        );
        // Plain and aliased aggregates keep their direct form and names.
        let plan = sql_to_logical_plan("SELECT sum(x), avg(y) AS a FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("project");
        };
        assert!(matches!(&columns[0], Expr::Function { name, .. } if name == "SUM"));
    }

    #[test]
    fn parses_simple_select() {
        let plan = sql_to_logical_plan("SELECT * FROM users").unwrap();
        match plan {
            LogicalPlan::Scan { table, .. } => assert_eq!(table, "users"),
            _ => panic!("expected Scan for SELECT *"),
        }
    }

    #[test]
    fn parses_select_with_columns() {
        let plan = sql_to_logical_plan("SELECT name, age FROM users").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => assert_eq!(columns.len(), 2),
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_where_clause() {
        let plan = sql_to_logical_plan("SELECT * FROM users WHERE age > 21").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => match predicate {
                Expr::BinaryOp { op, .. } => assert_eq!(op, BinaryOp::Gt),
                _ => panic!("expected BinaryOp"),
            },
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_aggregate() {
        let plan = sql_to_logical_plan("SELECT city, COUNT(*) FROM users GROUP BY city").unwrap();
        match plan {
            LogicalPlan::Project { input, .. } => match *input {
                LogicalPlan::Aggregate {
                    group_by,
                    aggregates,
                    ..
                } => {
                    assert_eq!(group_by.len(), 1);
                    assert_eq!(aggregates.len(), 1);
                }
                _ => panic!("expected Aggregate under Project"),
            },
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_order_by() {
        let plan = sql_to_logical_plan("SELECT * FROM users ORDER BY name ASC, age DESC").unwrap();
        match plan {
            LogicalPlan::Sort { order_by, .. } => {
                assert_eq!(order_by.len(), 2);
                assert!(order_by[0].1);
                assert!(!order_by[1].1);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn parses_limit() {
        let plan = sql_to_logical_plan("SELECT * FROM users LIMIT 10").unwrap();
        match plan {
            LogicalPlan::Limit { count, .. } => assert_eq!(count, 10),
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn parses_complex_query() {
        let plan = sql_to_logical_plan(
            "SELECT city, SUM(amount) FROM orders WHERE status = 'completed' GROUP BY city ORDER BY city LIMIT 5",
        )
        .unwrap();
        match plan {
            LogicalPlan::Limit { count: 5, input } => match *input {
                LogicalPlan::Sort { input, .. } => match *input {
                    LogicalPlan::Project { input, .. } => match *input {
                        LogicalPlan::Aggregate { input, .. } => match *input {
                            LogicalPlan::Filter { input, .. } => match *input {
                                LogicalPlan::Scan { table, .. } => {
                                    assert_eq!(table, "orders");
                                }
                                _ => panic!("expected Scan"),
                            },
                            _ => panic!("expected Filter"),
                        },
                        _ => panic!("expected Aggregate"),
                    },
                    _ => panic!("expected Project"),
                },
                _ => panic!("expected Sort"),
            },
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn rejects_empty_query() {
        assert!(sql_to_logical_plan("").is_err());
    }

    #[test]
    fn rejects_insert() {
        assert!(sql_to_logical_plan("INSERT INTO users VALUES (1)").is_err());
    }

    #[test]
    fn preserves_count_distinct_semantics() {
        // A lone COUNT(DISTINCT x) counts the distinct x rows instead.
        let plan = sql_to_logical_plan("SELECT COUNT(DISTINCT user_id) FROM events").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate {
            aggregates, input, ..
        } = *input
        else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Count { distinct: false, expr: Expr::Column(column) }] if column == "user_id"
        ));
        let LogicalPlan::Distinct { input } = *input else {
            panic!("distinct rows feed the count");
        };
        assert!(matches!(*input, LogicalPlan::Project { ref columns, .. }
            if columns == &[Expr::Column("user_id".into())]));
        // Mixed with another aggregate, the exact distinct state remains.
        let plan =
            sql_to_logical_plan("SELECT k, COUNT(DISTINCT user_id), SUM(v) FROM events GROUP BY k")
                .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [
                AggregateExpr::Count { distinct: true, .. },
                AggregateExpr::Sum { .. }
            ]
        ));
    }

    #[test]
    fn plans_supported_join_types() {
        for (keyword, expected) in [
            ("INNER JOIN", JoinType::Inner),
            ("LEFT JOIN", JoinType::Left),
            ("RIGHT JOIN", JoinType::Right),
            ("FULL JOIN", JoinType::Full),
        ] {
            let sql = format!("SELECT * FROM users u {keyword} orders o ON u.id = o.user_id");
            let LogicalPlan::Join {
                join_type,
                condition,
                ..
            } = sql_to_logical_plan(&sql).unwrap()
            else {
                panic!("expected join")
            };
            assert_eq!(join_type, expected);
            assert!(condition.is_some());
        }
        let LogicalPlan::Join {
            join_type,
            condition,
            ..
        } = sql_to_logical_plan("SELECT * FROM users CROSS JOIN regions").unwrap()
        else {
            panic!("expected cross join")
        };
        assert_eq!(join_type, JoinType::Cross);
        assert!(condition.is_none());
    }

    #[test]
    fn parses_case_when() {
        let plan = sql_to_logical_plan("SELECT CASE WHEN x > 1 THEN 'big' ELSE 'small' END FROM t")
            .unwrap();
        assert!(matches!(plan, LogicalPlan::Project { .. }));
    }

    #[test]
    fn parses_like() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name LIKE '%foo%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Like { negated: false, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_not_like() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name NOT LIKE 'a%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Like { negated: true, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_between() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE x BETWEEN 1 AND 10").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Between { negated: false, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_in_list() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE x IN (1, 2, 3)").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(
                    predicate,
                    Expr::InList {
                        negated: false,
                        ref list,
                        ..
                    } if list.len() == 3
                ));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_cast() {
        let plan = sql_to_logical_plan("SELECT CAST(x AS BIGINT) FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert!(matches!(
                    columns[0],
                    Expr::Cast {
                        data_type: CastTarget::Int64,
                        ..
                    }
                ));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_having() {
        let plan =
            sql_to_logical_plan("SELECT city, COUNT(*) FROM t GROUP BY city HAVING COUNT(*) > 5")
                .unwrap();
        match plan {
            LogicalPlan::Project { input, .. } => match *input {
                LogicalPlan::Filter { input, .. } => {
                    assert!(matches!(*input, LogicalPlan::Aggregate { .. }));
                }
                _ => panic!("expected Filter (HAVING) after Aggregate"),
            },
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn having_aggregates_are_computed_even_when_the_projection_does_not_select_them() {
        let plan = sql_to_logical_plan(
            "SELECT l_orderkey FROM lineitem GROUP BY l_orderkey HAVING sum(l_quantity) > 300",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        assert_eq!(columns, vec![Expr::Column("l_orderkey".into())]);
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("HAVING filter");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
        let LogicalPlan::Aggregate {
            group_by,
            aggregates,
            ..
        } = *input
        else {
            panic!("an aggregate, not the DISTINCT a bare GROUP BY lowers to");
        };
        assert_eq!(group_by, vec![Expr::Column("l_orderkey".into())]);
        assert_eq!(
            aggregates,
            vec![AggregateExpr::Sum {
                expr: Expr::Column("l_quantity".into()),
                distinct: false
            }]
        );
        // An aggregate the projection already carries is computed once.
        let plan = sql_to_logical_plan(
            "SELECT k, SUM(x) AS s FROM t GROUP BY k HAVING SUM(x) > 1 AND COUNT(*) > 2",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, .. } = *input else {
            panic!("HAVING filter");
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("aggregate");
        };
        assert_eq!(
            aggregates,
            vec![
                AggregateExpr::Sum {
                    expr: Expr::Column("x".into()),
                    distinct: false
                },
                AggregateExpr::Count {
                    expr: Expr::Star,
                    distinct: false
                }
            ]
        );
    }

    #[test]
    fn a_scalar_subquery_in_where_is_a_single_row_join() {
        let plan =
            sql_to_logical_plan("SELECT a FROM t WHERE b > (SELECT avg(b) FROM t WHERE c = 1)")
                .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("filter");
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(Expr::Column("b".into())),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Column("__kaveon_scalar_0".into())),
            }
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type: JoinType::Cross,
            condition: None,
            ..
        } = *input
        else {
            panic!("cross join with the one-row aggregate");
        };
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("the subquery projects its one column under the scalar's name");
        };
        assert_eq!(
            columns,
            vec![Expr::Alias {
                expr: Box::new(Expr::Function {
                    name: "AVG".into(),
                    args: vec![Expr::Column("b".into())]
                }),
                name: "__kaveon_scalar_0".into()
            }]
        );
        assert!(matches!(*input, LogicalPlan::Aggregate { .. }));
        // Two scalars take distinct names.
        let plan = sql_to_logical_plan(
            "SELECT a FROM t WHERE b > (SELECT avg(b) FROM t) AND c < (SELECT max(c) FROM u)",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("filter");
        };
        let Expr::And(_, second) = predicate else {
            panic!("both conjuncts");
        };
        assert!(matches!(
            *second,
            Expr::BinaryOp { right, .. } if *right == Expr::Column("__kaveon_scalar_1".into())
        ));
        // A subquery that could yield several rows is refused.
        for sql in [
            "SELECT a FROM t WHERE b > (SELECT b FROM t)",
            "SELECT a FROM t WHERE b > (SELECT max(b) FROM t GROUP BY c)",
            "SELECT a FROM t WHERE b > (SELECT max(b), min(b) FROM t)",
        ] {
            assert!(sql_to_logical_plan(sql).is_err(), "{sql}");
        }
        assert!(sql_to_logical_plan("SELECT (SELECT max(b) FROM t) FROM t").is_err());
    }

    #[test]
    fn a_scalar_subquery_in_having_joins_the_aggregate_output() {
        let plan = sql_to_logical_plan(
            "SELECT k, sum(v) AS total FROM t GROUP BY k HAVING sum(v) > (SELECT sum(v) * 0.5 FROM t) AND count(*) > 1 ORDER BY total DESC",
        )
        .unwrap();
        let LogicalPlan::Sort { input, .. } = plan else {
            panic!("sort");
        };
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("projection");
        };
        // Every aggregate is a named column above the join.
        assert_eq!(
            columns[1],
            Expr::Alias {
                expr: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                name: "total".into()
            }
        );
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("HAVING filter");
        };
        assert_eq!(
            predicate,
            Expr::And(
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Column("__kaveon_scalar_0".into())),
                }),
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("count_*".into())),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(1))),
                }),
            )
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type: JoinType::Cross,
            ..
        } = *input
        else {
            panic!("the scalar joins the aggregate output");
        };
        assert!(matches!(*left, LogicalPlan::Aggregate { .. }));
        let LogicalPlan::Project { columns, .. } = *right else {
            panic!("scalar projection");
        };
        assert!(matches!(&columns[0], Expr::Alias { name, .. } if name == "__kaveon_scalar_0"));
    }

    #[test]
    fn parses_distinct() {
        let plan = sql_to_logical_plan("SELECT DISTINCT city FROM t").unwrap();
        match plan {
            LogicalPlan::Distinct { input } => {
                assert!(matches!(*input, LogicalPlan::Project { .. }));
            }
            _ => panic!("expected Distinct"),
        }
    }

    #[test]
    fn parses_offset() {
        let plan = sql_to_logical_plan("SELECT * FROM t LIMIT 10 OFFSET 5").unwrap();
        match plan {
            LogicalPlan::Limit { input, count: 10 } => match *input {
                LogicalPlan::Offset { count: 5, .. } => {}
                _ => panic!("expected Offset under Limit"),
            },
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn parses_union_all() {
        let plan = sql_to_logical_plan("SELECT a FROM t1 UNION ALL SELECT b FROM t2").unwrap();
        assert!(matches!(plan, LogicalPlan::Union { all: true, .. }));
    }

    #[test]
    fn parses_cte() {
        let plan = sql_to_logical_plan(
            "WITH active AS (SELECT * FROM users WHERE status = 'active') SELECT * FROM active",
        )
        .unwrap();
        match plan {
            LogicalPlan::Filter { .. } => {}
            _ => panic!("expected CTE to inline as Filter (from WHERE in CTE body)"),
        }
    }

    #[test]
    fn parses_string_concat_operator() {
        let plan = sql_to_logical_plan("SELECT a || b FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert!(matches!(
                    columns[0],
                    Expr::BinaryOp {
                        op: BinaryOp::StringConcat,
                        ..
                    }
                ));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_ilike() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name ILIKE '%foo%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(
                    predicate,
                    Expr::Like {
                        case_insensitive: true,
                        ..
                    }
                ));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_scalar_functions() {
        let plan = sql_to_logical_plan("SELECT UPPER(name), LENGTH(name) FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert_eq!(columns.len(), 2);
                assert!(matches!(columns[0], Expr::Function { ref name, .. } if name == "UPPER"));
                assert!(matches!(columns[1], Expr::Function { ref name, .. } if name == "LENGTH"));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_window_frame_rows_between() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(amount) OVER (ORDER BY id ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) FROM t",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Window { window_exprs, .. } = *input else {
            panic!("expected Window");
        };
        assert_eq!(window_exprs.len(), 1);
        let Expr::WindowFunction { frame, .. } = &window_exprs[0] else {
            panic!("expected WindowFunction");
        };
        let frame = frame.as_ref().expect("frame should be Some");
        assert_eq!(frame.units, WindowFrameUnits::Rows);
        assert_eq!(frame.start, WindowFrameBound::Preceding(2));
        assert_eq!(frame.end, WindowFrameBound::CurrentRow);
    }

    #[test]
    fn parses_window_frame_unbounded() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(x) OVER (PARTITION BY g ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Window { window_exprs, .. } = *input else {
            panic!("expected Window");
        };
        let Expr::WindowFunction { frame, .. } = &window_exprs[0] else {
            panic!("expected WindowFunction");
        };
        let frame = frame.as_ref().expect("frame should be Some");
        assert_eq!(frame.start, WindowFrameBound::UnboundedPreceding);
        assert_eq!(frame.end, WindowFrameBound::UnboundedFollowing);
    }

    #[test]
    fn parses_decimal_literal() {
        let plan = sql_to_logical_plan("SELECT 123.45 FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        match &columns[0] {
            Expr::Literal(ScalarValue::Decimal128 {
                value,
                precision,
                scale,
            }) => {
                assert_eq!(*value, 12345);
                assert_eq!(*scale, 2);
                assert!(*precision >= 5);
            }
            other => panic!("expected Decimal128 literal, got {other:?}"),
        }
    }

    #[test]
    fn parses_cast_to_decimal() {
        let plan = sql_to_logical_plan("SELECT CAST(x AS DECIMAL(10,2)) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        assert!(matches!(
            columns[0],
            Expr::Cast {
                data_type: CastTarget::Decimal128 {
                    precision: 10,
                    scale: 2,
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_in_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM orders WHERE user_id IN (SELECT id FROM users WHERE active = true)",
        )
        .unwrap();
        assert!(matches!(plan, LogicalPlan::SemiJoin { .. }));
    }

    #[test]
    fn parses_not_in_subquery() {
        let plan =
            sql_to_logical_plan("SELECT * FROM orders WHERE user_id NOT IN (SELECT id FROM users)")
                .unwrap();
        assert!(matches!(plan, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn parses_exists_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM orders WHERE EXISTS (SELECT NULL FROM users WHERE users.id = 1)",
        )
        .unwrap();
        assert!(matches!(plan, LogicalPlan::SemiJoin { .. }));
    }

    #[test]
    fn rejects_correlated_subqueries_instead_of_rebinding_outer_columns() {
        for sql in [
            "SELECT * FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
            "SELECT * FROM orders o WHERE o.id IN (SELECT u.id FROM users u WHERE u.id = o.id)",
        ] {
            assert!(
                sql_to_logical_plan(sql)
                    .unwrap_err()
                    .to_string()
                    .contains("correlated")
            );
        }
    }

    #[test]
    fn rejects_unimplemented_window_modifiers() {
        for sql in [
            "SELECT COUNT(DISTINCT x) OVER () FROM t",
            "SELECT COUNT(*) OVER (ORDER BY x NULLS FIRST) FROM t",
        ] {
            assert!(sql_to_logical_plan(sql).is_err());
        }
    }

    #[test]
    fn parses_not_exists_subquery() {
        let plan =
            sql_to_logical_plan("SELECT * FROM orders WHERE NOT EXISTS (SELECT 1 FROM users)")
                .unwrap();
        assert!(matches!(plan, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn parses_sum_distinct() {
        let plan = sql_to_logical_plan("SELECT SUM(DISTINCT amount) FROM t").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Sum { distinct: true, .. }]
        ));
    }

    #[test]
    fn parses_avg_distinct() {
        let plan = sql_to_logical_plan("SELECT AVG(DISTINCT score) FROM t").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Avg { distinct: true, .. }]
        ));
    }
}
