use std::collections::HashMap;

use crate::parser::parse_sql;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{BinaryOp, CastTarget, DateField, Expr, KaveonError, Result};
use sqlparser::ast;

#[derive(Debug)]
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
        ast::Statement::Query(query) => query_to_plan(query, &HashMap::new()),
        _ => Err(sql_err("only SELECT queries are supported")),
    }
}

fn query_to_plan(
    query: &ast::Query,
    parent_ctes: &HashMap<String, ast::Query>,
) -> Result<LogicalPlan> {
    let mut ctes = parent_ctes.clone();
    if let Some(with) = &query.with {
        if with.recursive {
            return Err(sql_err("recursive CTEs are not supported"));
        }
        for cte in &with.cte_tables {
            let cte_name = cte.alias.name.value.to_lowercase();
            ctes.insert(cte_name, *cte.query.clone());
        }
    }

    let plan = set_expr_to_plan(query.body.as_ref(), &ctes)?;

    let plan = match &query.order_by {
        Some(ob) => build_order_by(plan, &ob.exprs)?,
        None => plan,
    };
    let plan = build_limit_offset(plan, &query.limit, &query.offset)?;

    Ok(plan)
}

fn set_expr_to_plan(
    body: &ast::SetExpr,
    ctes: &HashMap<String, ast::Query>,
) -> Result<LogicalPlan> {
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
                    let all = matches!(
                        set_quantifier,
                        ast::SetQuantifier::All | ast::SetQuantifier::None
                    );
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

fn select_to_plan(select: &ast::Select, ctes: &HashMap<String, ast::Query>) -> Result<LogicalPlan> {
    let plan = build_from_clause(select, ctes)?;
    let plan = build_where(plan, select)?;

    let has_aggregates = select.projection.iter().any(contains_aggregate_select_item)
        || matches!(&select.group_by, ast::GroupByExpr::Expressions(exprs, _) if !exprs.is_empty());

    let plan = if has_aggregates {
        build_aggregate(plan, select)?
    } else {
        plan
    };

    let plan = build_having(plan, select)?;

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

    Ok(plan)
}

fn build_from_clause(
    select: &ast::Select,
    ctes: &HashMap<String, ast::Query>,
) -> Result<LogicalPlan> {
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
        };
    }
    Ok(plan)
}

fn table_factor_to_plan(
    factor: &ast::TableFactor,
    ctes: &HashMap<String, ast::Query>,
) -> Result<LogicalPlan> {
    match factor {
        ast::TableFactor::Table { name, alias, .. } => {
            let table_name = name.to_string();
            let alias_name = alias.as_ref().map(|a| a.name.value.clone());
            let lookup = alias_name.as_deref().unwrap_or(&table_name).to_lowercase();
            if let Some(cte_query) = ctes
                .get(&table_name.to_lowercase())
                .or_else(|| ctes.get(&lookup))
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
                if alias_name.is_some() {
                    if let LogicalPlan::Scan { ref mut alias, .. } = plan {
                        *alias = alias_name.clone();
                    }
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

fn apply_joins(
    mut left: LogicalPlan,
    joins: &[ast::Join],
    ctes: &HashMap<String, ast::Query>,
) -> Result<LogicalPlan> {
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
        };
    }
    Ok(left)
}

fn build_where(plan: LogicalPlan, select: &ast::Select) -> Result<LogicalPlan> {
    match &select.selection {
        None => Ok(plan),
        Some(expr) => {
            let predicate = ast_expr_to_expr(expr)?;
            Ok(LogicalPlan::Filter {
                input: Box::new(plan),
                predicate,
            })
        }
    }
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

    Ok(LogicalPlan::Aggregate {
        input: Box::new(plan),
        group_by,
        aggregates,
    })
}

fn build_having(plan: LogicalPlan, select: &ast::Select) -> Result<LogicalPlan> {
    match &select.having {
        None => Ok(plan),
        Some(expr) => {
            let predicate = ast_expr_to_expr(expr)?;
            Ok(LogicalPlan::Filter {
                input: Box::new(plan),
                predicate,
            })
        }
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

fn build_order_by(plan: LogicalPlan, order_by: &[ast::OrderByExpr]) -> Result<LogicalPlan> {
    if order_by.is_empty() {
        return Ok(plan);
    }
    let mut items = Vec::new();
    for ob in order_by {
        let expr = ast_expr_to_expr(&ob.expr)?;
        let asc = ob.asc.unwrap_or(true);
        items.push((expr, asc));
    }
    Ok(LogicalPlan::Sort {
        input: Box::new(plan),
        order_by: items,
    })
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
        ast::Expr::InSubquery { .. } => {
            Err(sql_err("IN (SELECT ...) subqueries are not yet supported"))
        }
        ast::Expr::Exists { .. } => Err(sql_err("EXISTS subqueries are not yet supported")),
        ast::Expr::Subquery(_) => Err(sql_err("scalar subqueries are not yet supported")),

        _ => Err(sql_err(format!("unsupported expression: {expr}"))),
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
        other => Err(sql_err(format!("unsupported CAST target type: {other}"))),
    }
}

fn ast_value_to_expr(value: &ast::Value) -> Result<Expr> {
    match value {
        ast::Value::Number(n, _) => {
            if let Ok(i) = n.parse::<i64>() {
                Ok(Expr::Literal(ScalarValue::Int64(i)))
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
        let (partition_by, order_by) = match over {
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
                        let expr = ast_expr_to_expr(&ob.expr)?;
                        let asc = ob.asc.unwrap_or(true);
                        Ok((expr, asc))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (partition_by, order_by)
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
        ast::Expr::Nested(inner) => collect_windows_from_ast_expr(inner, out),
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

fn sql_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Sql(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let plan = sql_to_logical_plan("SELECT COUNT(DISTINCT user_id) FROM events").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Count { distinct: true, .. }]
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
}
