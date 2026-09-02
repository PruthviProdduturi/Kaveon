use crate::parser::parse_sql;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{BinaryOp, Expr, KaveonError, Result};
use sqlparser::ast;

#[derive(Debug)]
pub enum AggregateExpr {
    Count(Expr),
    Sum(Expr),
    Avg(Expr),
    Min(Expr),
    Max(Expr),
}

#[derive(Debug)]
pub enum LogicalPlan {
    Scan {
        table: String,
        columns: Option<Vec<String>>,
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
        ast::Statement::Query(query) => query_to_plan(query),
        _ => Err(sql_err("only SELECT queries are supported")),
    }
}

fn query_to_plan(query: &ast::Query) -> Result<LogicalPlan> {
    let select = match query.body.as_ref() {
        ast::SetExpr::Select(select) => select,
        _ => return Err(sql_err("only simple SELECT queries are supported")),
    };

    let plan = build_from_clause(select)?;
    let plan = build_where(plan, select)?;

    let has_aggregates = select.projection.iter().any(contains_aggregate_select_item)
        || matches!(&select.group_by, ast::GroupByExpr::Expressions(exprs, _) if !exprs.is_empty());

    let plan = if has_aggregates {
        build_aggregate(plan, select)?
    } else {
        plan
    };

    let plan = build_projection(plan, select, has_aggregates)?;
    let plan = match &query.order_by {
        Some(ob) => build_order_by(plan, &ob.exprs)?,
        None => plan,
    };
    let plan = build_limit(plan, &query.limit, &query.offset)?;

    Ok(plan)
}

fn build_from_clause(select: &ast::Select) -> Result<LogicalPlan> {
    if select.from.is_empty() {
        return Err(sql_err("SELECT requires a FROM clause"));
    }
    if select.from.len() > 1 {
        return Err(sql_err("joins are not yet supported"));
    }
    let from = &select.from[0];
    if !from.joins.is_empty() {
        return Err(sql_err("joins are not yet supported"));
    }
    match &from.relation {
        ast::TableFactor::Table { name, .. } => {
            let table = name.to_string();
            Ok(LogicalPlan::Scan {
                table,
                columns: None,
            })
        }
        _ => Err(sql_err("only table references are supported in FROM")),
    }
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

fn build_limit(
    plan: LogicalPlan,
    limit: &Option<ast::Expr>,
    _offset: &Option<ast::Offset>,
) -> Result<LogicalPlan> {
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
        _ => Err(sql_err(format!("unsupported expression: {expr}"))),
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
            let name = func.name.to_string().to_uppercase();
            matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            contains_aggregate_expr(left) || contains_aggregate_expr(right)
        }
        ast::Expr::UnaryOp { expr, .. } => contains_aggregate_expr(expr),
        ast::Expr::Nested(inner) => contains_aggregate_expr(inner),
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
                let agg = match name.as_str() {
                    "COUNT" => AggregateExpr::Count(arg),
                    "SUM" => AggregateExpr::Sum(arg),
                    "AVG" => AggregateExpr::Avg(arg),
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
        _ => Ok(()),
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
}
