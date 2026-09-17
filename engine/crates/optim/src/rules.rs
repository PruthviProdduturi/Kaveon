use std::collections::{HashMap, HashSet};

use kaveon_core::{BinaryOp, CompareOp, Expr, ScalarValue, StoragePredicate};
use kaveon_sql::logical_plan::{JoinType, LogicalPlan};

pub fn push_filter_down(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Filter { input, predicate } => {
            let input = push_filter_down(*input);
            push_filter_into(predicate, input)
        }
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input: Box::new(push_filter_down(*input)),
            columns,
        },
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => LogicalPlan::Aggregate {
            input: Box::new(push_filter_down(*input)),
            group_by,
            aggregates,
        },
        LogicalPlan::Sort { input, order_by } => LogicalPlan::Sort {
            input: Box::new(push_filter_down(*input)),
            order_by,
        },
        LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
            input: Box::new(push_filter_down(*input)),
            count,
        },
        LogicalPlan::Offset { input, count } => LogicalPlan::Offset {
            input: Box::new(push_filter_down(*input)),
            count,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(push_filter_down(*input)),
        },
        LogicalPlan::Window {
            input,
            window_exprs,
        } => LogicalPlan::Window {
            input: Box::new(push_filter_down(*input)),
            window_exprs,
        },
        LogicalPlan::Union { inputs, all } => LogicalPlan::Union {
            inputs: inputs.into_iter().map(push_filter_down).collect(),
            all,
        },
        LogicalPlan::Intersect { left, right } => LogicalPlan::Intersect {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
        },
        LogicalPlan::Except { left, right } => LogicalPlan::Except {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
        },
        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            distribution,
        } => LogicalPlan::Join {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
            join_type,
            condition,
            distribution,
        },
        LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
        } => LogicalPlan::SemiJoin {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
            left_key,
            right_key,
        },
        LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
        } => LogicalPlan::AntiJoin {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
            left_key,
            right_key,
        },
        scan @ LogicalPlan::Scan { .. } => scan,
    }
}

pub fn push_projection_down(plan: LogicalPlan) -> LogicalPlan {
    prune_columns(plan, None)
}

fn prune_columns(plan: LogicalPlan, required: Option<HashSet<String>>) -> LogicalPlan {
    match plan {
        LogicalPlan::Scan {
            table,
            alias,
            columns,
        } => {
            let projected = required
                .filter(|columns| !columns.is_empty())
                .map(|columns| {
                    let mut columns = columns
                        .into_iter()
                        .map(|column| column.rsplit('.').next().unwrap_or(&column).to_owned())
                        .collect::<Vec<_>>();
                    columns.sort();
                    columns.dedup();
                    columns
                })
                .or(columns);
            LogicalPlan::Scan {
                table,
                alias,
                columns: projected,
            }
        }
        // A node asked for everything (`required` None) asks its input
        // for everything; only a named set of columns grows by what the
        // node itself reads.
        LogicalPlan::Filter { input, predicate } => {
            let required = required.map(|mut columns| {
                collect_columns(&predicate, &mut columns);
                columns
            });
            LogicalPlan::Filter {
                input: Box::new(prune_columns(*input, required)),
                predicate,
            }
        }
        LogicalPlan::Project { input, columns } => {
            let mut input_columns = HashSet::new();
            for expression in &columns {
                collect_columns(expression, &mut input_columns);
            }
            LogicalPlan::Project {
                input: Box::new(prune_columns(*input, Some(input_columns))),
                columns,
            }
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } => {
            let mut input_columns = HashSet::new();
            for expression in &group_by {
                collect_columns(expression, &mut input_columns);
            }
            for aggregate in &aggregates {
                let expression = match aggregate {
                    kaveon_sql::logical_plan::AggregateExpr::Count { expr, .. }
                    | kaveon_sql::logical_plan::AggregateExpr::Sum { expr, .. }
                    | kaveon_sql::logical_plan::AggregateExpr::Avg { expr, .. }
                    | kaveon_sql::logical_plan::AggregateExpr::Min(expr)
                    | kaveon_sql::logical_plan::AggregateExpr::Max(expr) => expr,
                };
                collect_columns(expression, &mut input_columns);
            }
            LogicalPlan::Aggregate {
                input: Box::new(prune_columns(*input, Some(input_columns))),
                group_by,
                aggregates,
            }
        }
        LogicalPlan::Sort { input, order_by } => {
            let required = required.map(|mut columns| {
                for (expression, _) in &order_by {
                    collect_columns(expression, &mut columns);
                }
                columns
            });
            LogicalPlan::Sort {
                input: Box::new(prune_columns(*input, required)),
                order_by,
            }
        }
        LogicalPlan::Limit { input, count } => LogicalPlan::Limit {
            input: Box::new(prune_columns(*input, required)),
            count,
        },
        LogicalPlan::Offset { input, count } => LogicalPlan::Offset {
            input: Box::new(prune_columns(*input, required)),
            count,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(prune_columns(*input, required)),
        },
        LogicalPlan::Window {
            input,
            window_exprs,
        } => {
            let required = required.map(|mut columns| {
                for expr in &window_exprs {
                    collect_columns(expr, &mut columns);
                }
                columns
            });
            LogicalPlan::Window {
                input: Box::new(prune_columns(*input, required)),
                window_exprs,
            }
        }
        LogicalPlan::Intersect { left, right } => LogicalPlan::Intersect {
            left: Box::new(prune_columns(*left, required.clone())),
            right: Box::new(prune_columns(*right, required)),
        },
        LogicalPlan::Except { left, right } => LogicalPlan::Except {
            left: Box::new(prune_columns(*left, required.clone())),
            right: Box::new(prune_columns(*right, required)),
        },
        LogicalPlan::Union { inputs, all } => LogicalPlan::Union {
            inputs: inputs
                .into_iter()
                .map(|p| prune_columns(p, required.clone()))
                .collect(),
            all,
        },
        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            distribution,
        } => {
            // Everything above is needed: every input keeps every column.
            let Some(mut columns) = required else {
                return LogicalPlan::Join {
                    left: Box::new(prune_columns(*left, None)),
                    right: Box::new(prune_columns(*right, None)),
                    join_type,
                    condition,
                    distribution,
                };
            };
            if let Some(condition) = &condition {
                collect_columns(condition, &mut columns);
            }
            // A qualified column goes to the side whose relations carry
            // its qualifier; a join of joins routes through every relation
            // beneath it. A bare column, or a side whose output the
            // relation names do not describe, keeps both sides whole.
            let sides = scan_qualifiers(&left).zip(scan_qualifiers(&right));
            let (left_required, right_required) = match sides {
                Some((left_qualifiers, right_qualifiers))
                    if !columns.is_empty()
                        && columns.iter().all(|column| {
                            column.rsplit_once('.').is_some_and(|(qualifier, _)| {
                                left_qualifiers.contains(qualifier)
                                    != right_qualifiers.contains(qualifier)
                            })
                        }) =>
                {
                    let (left_columns, right_columns): (HashSet<_>, HashSet<_>) =
                        columns.into_iter().partition(|column| {
                            column
                                .rsplit_once('.')
                                .is_some_and(|(qualifier, _)| left_qualifiers.contains(qualifier))
                        });
                    (Some(left_columns), Some(right_columns))
                }
                _ => (None, None),
            };
            LogicalPlan::Join {
                left: Box::new(prune_columns(*left, left_required)),
                right: Box::new(prune_columns(*right, right_required)),
                join_type,
                condition,
                distribution,
            }
        }
        // A semi or anti join emits its left input's rows, which need what
        // is required above plus the probe key; the subquery side names
        // its own output (its projection prunes beneath it), and a bare
        // scan there is an existence test that reads what it reads.
        LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
        } => {
            let left_required = required.map(|mut columns| {
                collect_columns(&left_key, &mut columns);
                columns
            });
            LogicalPlan::SemiJoin {
                left: Box::new(prune_columns(*left, left_required)),
                right: Box::new(prune_columns(*right, None)),
                left_key,
                right_key,
            }
        }
        LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
        } => {
            let left_required = required.map(|mut columns| {
                collect_columns(&left_key, &mut columns);
                columns
            });
            LogicalPlan::AntiJoin {
                left: Box::new(prune_columns(*left, left_required)),
                right: Box::new(prune_columns(*right, None)),
                left_key,
                right_key,
            }
        }
    }
}

fn collect_columns(expression: &Expr, columns: &mut HashSet<String>) {
    match expression {
        Expr::Column(name) => {
            columns.insert(name.to_owned());
        }
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            collect_columns(left, columns);
            collect_columns(right, columns);
        }
        Expr::IsNull(expression)
        | Expr::IsNotNull(expression)
        | Expr::Not(expression)
        | Expr::Alias {
            expr: expression, ..
        }
        | Expr::Cast {
            expr: expression, ..
        } => collect_columns(expression, columns),
        Expr::Function { args, .. } => {
            for argument in args {
                collect_columns(argument, columns);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                collect_columns(op, columns);
            }
            for (when, then) in when_then {
                collect_columns(when, columns);
                collect_columns(then, columns);
            }
            if let Some(e) = else_expr {
                collect_columns(e, columns);
            }
        }
        Expr::Like { expr, pattern, .. } => {
            collect_columns(expr, columns);
            collect_columns(pattern, columns);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_columns(expr, columns);
            collect_columns(low, columns);
            collect_columns(high, columns);
        }
        Expr::InList { expr, list, .. } => {
            collect_columns(expr, columns);
            for item in list {
                collect_columns(item, columns);
            }
        }
        Expr::WindowFunction {
            args,
            partition_by,
            order_by,
            ..
        } => {
            for arg in args {
                collect_columns(arg, columns);
            }
            for expr in partition_by {
                collect_columns(expr, columns);
            }
            for (expr, _) in order_by {
                collect_columns(expr, columns);
            }
        }
        Expr::Extract { expr, .. } => collect_columns(expr, columns),
        Expr::Literal(_) | Expr::Star => {}
    }
}

/// The relations a join qualifies the columns of `plan` with: scans (and
/// filtered scans) under their alias or table name, through nested joins.
/// None when an input names its own output (a projection or aggregate),
/// which the relation names do not describe.
fn scan_qualifiers(plan: &LogicalPlan) -> Option<HashSet<String>> {
    match plan {
        LogicalPlan::Scan { .. } | LogicalPlan::Filter { .. } => {
            plan_qualifier(plan).map(|qualifier| HashSet::from([qualifier]))
        }
        LogicalPlan::Join { left, right, .. } => {
            let mut qualifiers = scan_qualifiers(left)?;
            qualifiers.extend(scan_qualifiers(right)?);
            Some(qualifiers)
        }
        _ => None,
    }
}

fn plan_qualifier(plan: &LogicalPlan) -> Option<String> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .clone()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned()),
        ),
        // A filter keeps its input's relation.
        LogicalPlan::Filter { input, .. } => plan_qualifier(input),
        _ => None,
    }
}

/// The qualifier every column of `expression` carries, when they all carry
/// the same one; None for unqualified or mixed references.
fn expression_qualifier(expression: &Expr) -> Option<String> {
    let mut columns = HashSet::new();
    collect_columns(expression, &mut columns);
    let mut qualifiers = columns.iter().map(|column| {
        column
            .rsplit_once('.')
            .map(|(qualifier, _)| qualifier.to_owned())
    });
    let first = qualifiers.next()??;
    qualifiers
        .all(|qualifier| qualifier.as_deref() == Some(first.as_str()))
        .then_some(first)
}

fn conjuncts(expression: Expr, into: &mut Vec<Expr>) {
    match expression {
        Expr::And(left, right) => {
            conjuncts(*left, into);
            conjuncts(*right, into);
        }
        other => into.push(other),
    }
}

fn conjoin(mut expressions: Vec<Expr>) -> Option<Expr> {
    let mut result = expressions.pop()?;
    while let Some(expression) = expressions.pop() {
        result = Expr::And(Box::new(expression), Box::new(result));
    }
    Some(result)
}

/// Columns qualified with the scan's own relation become bare column names,
/// which is what storage predicates and the row filter resolve.
fn strip_qualifier(expression: Expr, qualifier: &str) -> Expr {
    let strip = |expr: Box<Expr>| Box::new(strip_qualifier(*expr, qualifier));
    match expression {
        Expr::Column(name) => Expr::Column(match name.rsplit_once('.') {
            Some((prefix, column)) if prefix == qualifier => column.to_owned(),
            _ => name,
        }),
        Expr::Literal(_) | Expr::Star => expression,
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: strip(left),
            op,
            right: strip(right),
        },
        Expr::And(left, right) => Expr::And(strip(left), strip(right)),
        Expr::Or(left, right) => Expr::Or(strip(left), strip(right)),
        Expr::IsNull(expr) => Expr::IsNull(strip(expr)),
        Expr::IsNotNull(expr) => Expr::IsNotNull(strip(expr)),
        Expr::Not(expr) => Expr::Not(strip(expr)),
        Expr::Alias { expr, name } => Expr::Alias {
            expr: strip(expr),
            name,
        },
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: strip(expr),
            data_type,
        },
        Expr::Extract { field, expr } => Expr::Extract {
            field,
            expr: strip(expr),
        },
        Expr::Function { name, args } => Expr::Function {
            name,
            args: args
                .into_iter()
                .map(|arg| strip_qualifier(arg, qualifier))
                .collect(),
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => Expr::Case {
            operand: operand.map(strip),
            when_then: when_then
                .into_iter()
                .map(|(when, then)| {
                    (
                        strip_qualifier(when, qualifier),
                        strip_qualifier(then, qualifier),
                    )
                })
                .collect(),
            else_expr: else_expr.map(strip),
        },
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: strip(expr),
            pattern: strip(pattern),
            negated,
            case_insensitive,
        },
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: strip(expr),
            low: strip(low),
            high: strip(high),
            negated,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => Expr::InList {
            expr: strip(expr),
            list: list
                .into_iter()
                .map(|item| strip_qualifier(item, qualifier))
                .collect(),
            negated,
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
                .map(|arg| strip_qualifier(arg, qualifier))
                .collect(),
            partition_by: partition_by
                .into_iter()
                .map(|expr| strip_qualifier(expr, qualifier))
                .collect(),
            order_by: order_by
                .into_iter()
                .map(|(expr, ascending)| (strip_qualifier(expr, qualifier), ascending))
                .collect(),
            frame,
        },
    }
}

pub fn to_storage_predicate(expr: &Expr) -> Option<StoragePredicate> {
    match expr {
        Expr::BinaryOp { left, op, right } => comparison(left, *op, right),
        Expr::IsNull(expr) => column_name(expr).map(|column| StoragePredicate::IsNull { column }),
        Expr::IsNotNull(expr) => {
            column_name(expr).map(|column| StoragePredicate::IsNotNull { column })
        }
        Expr::And(left, right) => match (to_storage_predicate(left), to_storage_predicate(right)) {
            (Some(left), Some(right)) => Some(StoragePredicate::And(vec![left, right])),
            (Some(predicate), None) | (None, Some(predicate)) => Some(predicate),
            (None, None) => None,
        },
        Expr::Or(left, right) => combine_predicates(left, right, StoragePredicate::Or),
        Expr::Not(expr) => {
            to_storage_predicate(expr).map(|predicate| StoragePredicate::Not(Box::new(predicate)))
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let column = column_name(expr)?;
            let values: Vec<ScalarValue> = list
                .iter()
                .filter_map(|e| match e {
                    Expr::Literal(v) if !matches!(v, ScalarValue::Null) => Some(v.clone()),
                    _ => None,
                })
                .collect();
            if values.len() != list.len() {
                return None;
            }
            let pred = StoragePredicate::In { column, values };
            if *negated {
                Some(StoragePredicate::Not(Box::new(pred)))
            } else {
                Some(pred)
            }
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            let col = column_name(expr)?;
            let lo_val = match low.as_ref() {
                Expr::Literal(v) if !matches!(v, ScalarValue::Null) => v.clone(),
                _ => return None,
            };
            let hi_val = match high.as_ref() {
                Expr::Literal(v) if !matches!(v, ScalarValue::Null) => v.clone(),
                _ => return None,
            };
            let pred = StoragePredicate::And(vec![
                StoragePredicate::Compare {
                    column: col.clone(),
                    op: CompareOp::Ge,
                    value: lo_val,
                },
                StoragePredicate::Compare {
                    column: col,
                    op: CompareOp::Le,
                    value: hi_val,
                },
            ]);
            if *negated {
                Some(StoragePredicate::Not(Box::new(pred)))
            } else {
                Some(pred)
            }
        }
        Expr::Column(_)
        | Expr::Literal(_)
        | Expr::Function { .. }
        | Expr::Star
        | Expr::Alias { .. }
        | Expr::Case { .. }
        | Expr::Like { .. }
        | Expr::Cast { .. }
        | Expr::WindowFunction { .. }
        | Expr::Extract { .. } => None,
    }
}

fn push_filter_into(predicate: Expr, input: LogicalPlan) -> LogicalPlan {
    match input {
        LogicalPlan::Project { input, columns } => {
            if let Some(rewritten) = rewrite_for_projection(&predicate, &columns) {
                LogicalPlan::Project {
                    input: Box::new(push_filter_into(rewritten, *input)),
                    columns,
                }
            } else {
                LogicalPlan::Filter {
                    input: Box::new(LogicalPlan::Project { input, columns }),
                    predicate,
                }
            }
        }
        LogicalPlan::Sort { input, order_by } => LogicalPlan::Sort {
            input: Box::new(push_filter_into(predicate, *input)),
            order_by,
        },
        LogicalPlan::Filter {
            input,
            predicate: inner,
        } => push_filter_into(Expr::And(Box::new(inner), Box::new(predicate)), *input),
        LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            distribution,
        } => {
            // A conjunct that references one side only moves below the join.
            // An outer join keeps the preserved side's rows whatever the
            // filter says about the other side's columns, so only the
            // preserved side takes filters.
            let (into_left, into_right) = match join_type {
                JoinType::Inner | JoinType::Cross => (true, true),
                JoinType::Left => (true, false),
                JoinType::Right => (false, true),
                JoinType::Full => (false, false),
            };
            let left_qualifier = plan_qualifier(&left);
            let right_qualifier = plan_qualifier(&right);
            let mut parts = Vec::new();
            conjuncts(predicate, &mut parts);
            let (mut left_parts, mut right_parts, mut remaining) = (vec![], vec![], vec![]);
            for part in parts {
                let qualifier = expression_qualifier(&part);
                if into_left && qualifier.is_some() && qualifier == left_qualifier {
                    left_parts.push(part);
                } else if into_right && qualifier.is_some() && qualifier == right_qualifier {
                    right_parts.push(part);
                } else {
                    remaining.push(part);
                }
            }
            let left = match conjoin(left_parts) {
                Some(predicate) => push_filter_into(predicate, *left),
                None => *left,
            };
            let right = match conjoin(right_parts) {
                Some(predicate) => push_filter_into(predicate, *right),
                None => *right,
            };
            let join = LogicalPlan::Join {
                left: Box::new(left),
                right: Box::new(right),
                join_type,
                condition,
                distribution,
            };
            match conjoin(remaining) {
                Some(predicate) => LogicalPlan::Filter {
                    input: Box::new(join),
                    predicate,
                },
                None => join,
            }
        }
        // A semi or anti join emits rows of its left input, so a filter on
        // its output is a filter on that input.
        LogicalPlan::SemiJoin {
            left,
            right,
            left_key,
            right_key,
        } => LogicalPlan::SemiJoin {
            left: Box::new(push_filter_into(predicate, *left)),
            right,
            left_key,
            right_key,
        },
        LogicalPlan::AntiJoin {
            left,
            right,
            left_key,
            right_key,
        } => LogicalPlan::AntiJoin {
            left: Box::new(push_filter_into(predicate, *left)),
            right,
            left_key,
            right_key,
        },
        LogicalPlan::Scan {
            table,
            alias,
            columns,
        } => {
            let qualifier = alias
                .clone()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(&table).to_owned());
            LogicalPlan::Filter {
                input: Box::new(LogicalPlan::Scan {
                    table,
                    alias,
                    columns,
                }),
                predicate: strip_qualifier(predicate, &qualifier),
            }
        }
        boundary => LogicalPlan::Filter {
            input: Box::new(boundary),
            predicate,
        },
    }
}

fn rewrite_for_projection(predicate: &Expr, columns: &[Expr]) -> Option<Expr> {
    let mut names = HashMap::new();
    for expression in columns {
        match expression {
            Expr::Column(name) => insert_unique(&mut names, name, name)?,
            Expr::Alias { expr, name } => {
                let source = column_name(expr)?;
                insert_unique(&mut names, name, &source)?;
            }
            _ => {}
        }
    }
    rewrite_columns(predicate, &names)
}

fn insert_unique(names: &mut HashMap<String, String>, output: &str, source: &str) -> Option<()> {
    if names.insert(output.to_owned(), source.to_owned()).is_some() {
        return None;
    }
    Some(())
}

fn rewrite_columns(expr: &Expr, names: &HashMap<String, String>) -> Option<Expr> {
    match expr {
        Expr::Column(name) => names.get(name).cloned().map(Expr::Column),
        Expr::Literal(value) => Some(Expr::Literal(value.clone())),
        Expr::BinaryOp { left, op, right } => Some(Expr::BinaryOp {
            left: Box::new(rewrite_columns(left, names)?),
            op: *op,
            right: Box::new(rewrite_columns(right, names)?),
        }),
        Expr::IsNull(expr) => Some(Expr::IsNull(Box::new(rewrite_columns(expr, names)?))),
        Expr::IsNotNull(expr) => Some(Expr::IsNotNull(Box::new(rewrite_columns(expr, names)?))),
        Expr::Not(expr) => Some(Expr::Not(Box::new(rewrite_columns(expr, names)?))),
        Expr::And(left, right) => Some(Expr::And(
            Box::new(rewrite_columns(left, names)?),
            Box::new(rewrite_columns(right, names)?),
        )),
        Expr::Or(left, right) => Some(Expr::Or(
            Box::new(rewrite_columns(left, names)?),
            Box::new(rewrite_columns(right, names)?),
        )),
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Some(Expr::Like {
            expr: Box::new(rewrite_columns(expr, names)?),
            pattern: Box::new(rewrite_columns(pattern, names)?),
            negated: *negated,
            case_insensitive: *case_insensitive,
        }),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Some(Expr::Between {
            expr: Box::new(rewrite_columns(expr, names)?),
            low: Box::new(rewrite_columns(low, names)?),
            high: Box::new(rewrite_columns(high, names)?),
            negated: *negated,
        }),
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let e = rewrite_columns(expr, names)?;
            let items: Option<Vec<Expr>> = list.iter().map(|i| rewrite_columns(i, names)).collect();
            Some(Expr::InList {
                expr: Box::new(e),
                list: items?,
                negated: *negated,
            })
        }
        Expr::Cast { expr, data_type } => Some(Expr::Cast {
            expr: Box::new(rewrite_columns(expr, names)?),
            data_type: *data_type,
        }),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let op = operand
                .as_ref()
                .and_then(|e| rewrite_columns(e, names).map(Box::new));
            let wt: Option<Vec<(Expr, Expr)>> = when_then
                .iter()
                .map(|(w, t)| Some((rewrite_columns(w, names)?, rewrite_columns(t, names)?)))
                .collect();
            let el = else_expr
                .as_ref()
                .and_then(|e| rewrite_columns(e, names).map(Box::new));
            Some(Expr::Case {
                operand: op,
                when_then: wt?,
                else_expr: el,
            })
        }
        Expr::Extract { field, expr } => Some(Expr::Extract {
            field: *field,
            expr: Box::new(rewrite_columns(expr, names)?),
        }),
        Expr::Function { .. } | Expr::Star | Expr::Alias { .. } | Expr::WindowFunction { .. } => {
            None
        }
    }
}

fn comparison(left: &Expr, op: BinaryOp, right: &Expr) -> Option<StoragePredicate> {
    let compare_op = to_compare_op(op)?;
    match (left, right) {
        (Expr::Column(column), Expr::Literal(value)) if !matches!(value, ScalarValue::Null) => {
            Some(StoragePredicate::Compare {
                column: column.clone(),
                op: compare_op,
                value: value.clone(),
            })
        }
        (Expr::Literal(value), Expr::Column(column)) if !matches!(value, ScalarValue::Null) => {
            Some(StoragePredicate::Compare {
                column: column.clone(),
                op: reverse_compare_op(compare_op),
                value: value.clone(),
            })
        }
        _ => None,
    }
}

fn to_compare_op(op: BinaryOp) -> Option<CompareOp> {
    match op {
        BinaryOp::Eq => Some(CompareOp::Eq),
        BinaryOp::Ne => Some(CompareOp::Ne),
        BinaryOp::Lt => Some(CompareOp::Lt),
        BinaryOp::Le => Some(CompareOp::Le),
        BinaryOp::Gt => Some(CompareOp::Gt),
        BinaryOp::Ge => Some(CompareOp::Ge),
        BinaryOp::Plus
        | BinaryOp::Minus
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo
        | BinaryOp::StringConcat => None,
    }
}

fn reverse_compare_op(op: CompareOp) -> CompareOp {
    match op {
        CompareOp::Eq => CompareOp::Eq,
        CompareOp::Ne => CompareOp::Ne,
        CompareOp::Lt => CompareOp::Gt,
        CompareOp::Le => CompareOp::Ge,
        CompareOp::Gt => CompareOp::Lt,
        CompareOp::Ge => CompareOp::Le,
    }
}

fn combine_predicates(
    left: &Expr,
    right: &Expr,
    combine: impl FnOnce(Vec<StoragePredicate>) -> StoragePredicate,
) -> Option<StoragePredicate> {
    Some(combine(vec![
        to_storage_predicate(left)?,
        to_storage_predicate(right)?,
    ]))
}

fn column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Column(column) => Some(column.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> Expr {
        Expr::Column(name.to_owned())
    }
    fn int(value: i64) -> Expr {
        Expr::Literal(ScalarValue::Int64(value))
    }
    fn compare(left: Expr, op: BinaryOp, right: Expr) -> Expr {
        Expr::BinaryOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }
    fn scan() -> LogicalPlan {
        LogicalPlan::Scan {
            table: "orders".to_owned(),
            alias: None,
            columns: None,
        }
    }

    #[test]
    fn converts_column_literal_comparison() {
        let predicate = to_storage_predicate(&compare(column("amount"), BinaryOp::Ge, int(10)));
        match predicate {
            Some(StoragePredicate::Compare {
                column,
                op: CompareOp::Ge,
                value: ScalarValue::Int64(10),
            }) => assert_eq!(column, "amount"),
            other => panic!("unexpected predicate: {other:?}"),
        }
    }

    #[test]
    fn reverses_literal_column_comparison() {
        let predicate = to_storage_predicate(&compare(int(10), BinaryOp::Lt, column("amount")));
        assert!(matches!(
            predicate,
            Some(StoragePredicate::Compare {
                op: CompareOp::Gt,
                ..
            })
        ));
    }

    #[test]
    fn converts_boolean_and_null_predicates() {
        let expression = Expr::Or(
            Box::new(Expr::And(
                Box::new(compare(column("amount"), BinaryOp::Gt, int(10))),
                Box::new(Expr::IsNotNull(Box::new(column("region")))),
            )),
            Box::new(Expr::Not(Box::new(Expr::IsNull(Box::new(column(
                "customer",
            )))))),
        );
        assert!(
            matches!(to_storage_predicate(&expression), Some(StoragePredicate::Or(predicates)) if predicates.len() == 2)
        );
    }

    #[test]
    fn rejects_unsupported_or_partially_supported_expressions() {
        let arithmetic = compare(
            compare(column("amount"), BinaryOp::Plus, int(1)),
            BinaryOp::Gt,
            int(10),
        );
        let partial_or = Expr::Or(
            Box::new(compare(column("amount"), BinaryOp::Gt, int(10))),
            Box::new(arithmetic.clone()),
        );
        let null_comparison = compare(
            column("amount"),
            BinaryOp::Eq,
            Expr::Literal(ScalarValue::Null),
        );
        assert!(to_storage_predicate(&arithmetic).is_none());
        assert!(to_storage_predicate(&partial_or).is_none());
        assert!(to_storage_predicate(&null_comparison).is_none());
        assert!(to_storage_predicate(&compare(column("a"), BinaryOp::Eq, column("b"))).is_none());
    }

    #[test]
    fn retains_safe_conjunct_when_other_conjunct_is_unsupported() {
        let supported = compare(column("amount"), BinaryOp::Gt, int(10));
        let unsupported = compare(
            compare(column("amount"), BinaryOp::Plus, int(1)),
            BinaryOp::Lt,
            int(100),
        );
        let predicate =
            to_storage_predicate(&Expr::And(Box::new(supported), Box::new(unsupported)));
        assert!(matches!(
            predicate,
            Some(StoragePredicate::Compare {
                column,
                op: CompareOp::Gt,
                ..
            }) if column == "amount"
        ));
    }

    #[test]
    fn pushes_filter_through_sort_to_scan_boundary() {
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Sort {
                input: Box::new(scan()),
                order_by: vec![(column("amount"), false)],
            }),
            predicate: compare(column("amount"), BinaryOp::Gt, int(10)),
        };
        match push_filter_down(plan) {
            LogicalPlan::Sort { input, .. } => match *input {
                LogicalPlan::Filter { input, .. } => {
                    assert!(matches!(*input, LogicalPlan::Scan { .. }))
                }
                other => panic!("expected filter below sort, got {other:?}"),
            },
            other => panic!("expected sort root, got {other:?}"),
        }
    }

    #[test]
    fn pushes_filter_through_direct_projection_and_rewrites_alias() {
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Project {
                input: Box::new(scan()),
                columns: vec![Expr::Alias {
                    expr: Box::new(column("amount")),
                    name: "total".to_owned(),
                }],
            }),
            predicate: compare(column("total"), BinaryOp::Gt, int(10)),
        };
        match push_filter_down(plan) {
            LogicalPlan::Project { input, .. } => match *input {
                LogicalPlan::Filter { predicate, input } => {
                    assert!(matches!(*input, LogicalPlan::Scan { .. }));
                    assert!(
                        matches!(predicate, Expr::BinaryOp { left, .. } if matches!(*left, Expr::Column(ref name) if name == "amount"))
                    );
                }
                other => panic!("expected filter below project, got {other:?}"),
            },
            other => panic!("expected project root, got {other:?}"),
        }
    }

    #[test]
    fn preserves_filter_above_computed_projection() {
        let project = LogicalPlan::Project {
            input: Box::new(scan()),
            columns: vec![Expr::Alias {
                expr: Box::new(compare(column("amount"), BinaryOp::Plus, int(1))),
                name: "adjusted".to_owned(),
            }],
        };
        let plan = LogicalPlan::Filter {
            input: Box::new(project),
            predicate: compare(column("adjusted"), BinaryOp::Gt, int(10)),
        };
        assert!(
            matches!(push_filter_down(plan), LogicalPlan::Filter { input, .. } if matches!(*input, LogicalPlan::Project { .. }))
        );
    }

    #[test]
    fn does_not_cross_limit_or_aggregate_boundaries() {
        let predicate = compare(column("amount"), BinaryOp::Gt, int(10));
        let limited = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Limit {
                input: Box::new(scan()),
                count: 5,
            }),
            predicate: predicate.clone(),
        };
        let aggregate = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Aggregate {
                input: Box::new(scan()),
                group_by: vec![column("region")],
                aggregates: Vec::new(),
            }),
            predicate,
        };
        assert!(
            matches!(push_filter_down(limited), LogicalPlan::Filter { input, .. } if matches!(*input, LogicalPlan::Limit { .. }))
        );
        assert!(
            matches!(push_filter_down(aggregate), LogicalPlan::Filter { input, .. } if matches!(*input, LogicalPlan::Aggregate { .. }))
        );
    }

    #[test]
    fn merges_adjacent_filters_without_dropping_residual_evaluation() {
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Filter {
                input: Box::new(scan()),
                predicate: compare(column("amount"), BinaryOp::Gt, int(10)),
            }),
            predicate: Expr::IsNotNull(Box::new(column("region"))),
        };
        match push_filter_down(plan) {
            LogicalPlan::Filter { predicate, input } => {
                assert!(matches!(*input, LogicalPlan::Scan { .. }));
                assert!(matches!(predicate, Expr::And(_, _)));
            }
            other => panic!("expected one residual filter, got {other:?}"),
        }
    }

    #[test]
    fn converts_in_list_to_storage_predicate() {
        let expr = Expr::InList {
            expr: Box::new(column("status")),
            list: vec![
                Expr::Literal(ScalarValue::Utf8("active".into())),
                Expr::Literal(ScalarValue::Utf8("pending".into())),
            ],
            negated: false,
        };
        match to_storage_predicate(&expr) {
            Some(StoragePredicate::In { column, values }) => {
                assert_eq!(column, "status");
                assert_eq!(values.len(), 2);
            }
            other => panic!("expected In predicate, got {other:?}"),
        }
    }

    #[test]
    fn converts_between_to_storage_predicate() {
        let expr = Expr::Between {
            expr: Box::new(column("amount")),
            low: Box::new(int(10)),
            high: Box::new(int(100)),
            negated: false,
        };
        assert!(matches!(
            to_storage_predicate(&expr),
            Some(StoragePredicate::And(_))
        ));
    }

    fn aliased(table: &str, alias: &str) -> LogicalPlan {
        LogicalPlan::Scan {
            table: format!("cat.schema.{table}"),
            alias: Some(alias.to_owned()),
            columns: None,
        }
    }
    fn join(left: LogicalPlan, right: LogicalPlan, join_type: JoinType) -> LogicalPlan {
        LogicalPlan::Join {
            left: Box::new(left),
            right: Box::new(right),
            join_type,
            condition: Some(compare(
                column("t.user_id"),
                BinaryOp::Eq,
                column("u.user_id"),
            )),
            distribution: kaveon_sql::logical_plan::JoinDistribution::Partitioned,
        }
    }
    fn filtered_scan(plan: &LogicalPlan) -> Option<(&Expr, &LogicalPlan)> {
        match plan {
            LogicalPlan::Filter { input, predicate }
                if matches!(**input, LogicalPlan::Scan { .. }) =>
            {
                Some((predicate, input))
            }
            _ => None,
        }
    }
    fn scan_columns<'a>(plan: &'a LogicalPlan, scans: &mut Vec<&'a Option<Vec<String>>>) {
        match plan {
            LogicalPlan::Scan { columns, .. } => scans.push(columns),
            LogicalPlan::Project { input, .. }
            | LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. } => scan_columns(input, scans),
            LogicalPlan::Join { left, right, .. } | LogicalPlan::SemiJoin { left, right, .. } => {
                scan_columns(left, scans);
                scan_columns(right, scans);
            }
            _ => {}
        }
    }

    #[test]
    fn strips_the_relation_qualifier_at_the_scan() {
        let plan = LogicalPlan::Filter {
            input: Box::new(aliased("events", "t")),
            predicate: Expr::And(
                Box::new(compare(column("t.day"), BinaryOp::Eq, int(7))),
                Box::new(Expr::Function {
                    name: "UPPER".to_owned(),
                    args: vec![column("t.surface")],
                }),
            ),
        };
        let pushed = push_filter_down(plan);
        let (predicate, _) = filtered_scan(&pushed).expect("filter over scan");
        assert_eq!(
            *predicate,
            Expr::And(
                Box::new(compare(column("day"), BinaryOp::Eq, int(7))),
                Box::new(Expr::Function {
                    name: "UPPER".to_owned(),
                    args: vec![column("surface")],
                }),
            )
        );
        // The bare table name is a qualifier too; a foreign qualifier stays.
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Scan {
                table: "cat.schema.events".to_owned(),
                alias: None,
                columns: None,
            }),
            predicate: compare(column("events.day"), BinaryOp::Eq, column("x.day")),
        };
        let pushed = push_filter_down(plan);
        let (predicate, _) = filtered_scan(&pushed).expect("filter over scan");
        assert_eq!(
            *predicate,
            compare(column("day"), BinaryOp::Eq, column("x.day"))
        );
    }

    #[test]
    fn splits_inner_join_filters_by_side_and_keeps_cross_side_conjuncts() {
        let plan = LogicalPlan::Filter {
            input: Box::new(join(
                aliased("events", "t"),
                aliased("users", "u"),
                JoinType::Inner,
            )),
            predicate: Expr::And(
                Box::new(Expr::And(
                    Box::new(compare(column("t.day"), BinaryOp::Eq, int(7))),
                    Box::new(compare(column("u.locale"), BinaryOp::Eq, int(1))),
                )),
                Box::new(Expr::And(
                    Box::new(compare(
                        column("t.country"),
                        BinaryOp::Ne,
                        column("u.country"),
                    )),
                    Box::new(compare(column("t.surface"), BinaryOp::Eq, int(2))),
                )),
            ),
        };
        let LogicalPlan::Filter { input, predicate } = push_filter_down(plan) else {
            panic!("cross-side conjunct stays above the join");
        };
        assert_eq!(
            predicate,
            compare(column("t.country"), BinaryOp::Ne, column("u.country"))
        );
        let LogicalPlan::Join { left, right, .. } = *input else {
            panic!("join below the residual filter");
        };
        let (left_predicate, _) = filtered_scan(&left).expect("left side filtered");
        assert_eq!(
            *left_predicate,
            Expr::And(
                Box::new(compare(column("day"), BinaryOp::Eq, int(7))),
                Box::new(compare(column("surface"), BinaryOp::Eq, int(2))),
            )
        );
        let (right_predicate, _) = filtered_scan(&right).expect("right side filtered");
        assert_eq!(
            *right_predicate,
            compare(column("locale"), BinaryOp::Eq, int(1))
        );
        // Projection pruning sees through the pushed filters to both scans.
        let pruned = push_projection_down(LogicalPlan::Project {
            input: Box::new(LogicalPlan::Filter {
                input: Box::new(LogicalPlan::Join {
                    left,
                    right,
                    join_type: JoinType::Inner,
                    condition: Some(compare(
                        column("t.user_id"),
                        BinaryOp::Eq,
                        column("u.user_id"),
                    )),
                    distribution: kaveon_sql::logical_plan::JoinDistribution::Partitioned,
                }),
                predicate,
            }),
            columns: vec![column("t.actions"), column("u.locale")],
        });
        let mut scans = Vec::new();
        scan_columns(&pruned, &mut scans);
        assert_eq!(
            scans,
            vec![
                &Some(vec![
                    "actions".to_owned(),
                    "country".to_owned(),
                    "day".to_owned(),
                    "surface".to_owned(),
                    "user_id".to_owned()
                ]),
                &Some(vec![
                    "country".to_owned(),
                    "locale".to_owned(),
                    "user_id".to_owned()
                ]),
            ]
        );
    }

    #[test]
    fn outer_joins_only_take_filters_on_the_preserved_side() {
        let predicate = Expr::And(
            Box::new(compare(column("t.day"), BinaryOp::Eq, int(7))),
            Box::new(compare(column("u.locale"), BinaryOp::Eq, int(1))),
        );
        let plan = LogicalPlan::Filter {
            input: Box::new(join(
                aliased("events", "t"),
                aliased("users", "u"),
                JoinType::Left,
            )),
            predicate: predicate.clone(),
        };
        let LogicalPlan::Filter {
            input,
            predicate: residual,
        } = push_filter_down(plan)
        else {
            panic!("right-side conjunct stays above a left join");
        };
        assert_eq!(residual, compare(column("u.locale"), BinaryOp::Eq, int(1)));
        let LogicalPlan::Join { left, right, .. } = *input else {
            panic!("join below the residual filter");
        };
        assert!(filtered_scan(&left).is_some());
        assert!(matches!(*right, LogicalPlan::Scan { .. }));
        let plan = LogicalPlan::Filter {
            input: Box::new(join(
                aliased("events", "t"),
                aliased("users", "u"),
                JoinType::Full,
            )),
            predicate,
        };
        let LogicalPlan::Filter { input, .. } = push_filter_down(plan) else {
            panic!("full join keeps every filter above");
        };
        let LogicalPlan::Join { left, right, .. } = *input else {
            panic!("join below the filter");
        };
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        assert!(matches!(*right, LogicalPlan::Scan { .. }));
    }

    #[test]
    fn select_star_under_a_filtered_top_n_keeps_every_scan_column() {
        // ClickBench q24: `SELECT * FROM hits WHERE URL LIKE '%google%'
        // ORDER BY EventTime LIMIT 10`. The scan must stay unpruned — the
        // filter's and the sort's columns are not the answer's columns.
        let plan = kaveon_sql::logical_plan::sql_to_logical_plan(
            "SELECT * FROM hits WHERE url LIKE '%google%' ORDER BY event_time LIMIT 10",
        )
        .unwrap();
        let pruned = push_projection_down(push_filter_down(plan));
        let mut scans = Vec::new();
        scan_columns(&pruned, &mut scans);
        assert_eq!(scans, vec![&None]);
    }

    #[test]
    fn pruning_routes_through_nested_joins_and_keeps_everything_when_everything_is_needed() {
        let inner = |condition: Expr| LogicalPlan::Join {
            left: Box::new(aliased("customer", "c")),
            right: Box::new(aliased("orders", "o")),
            join_type: JoinType::Inner,
            condition: Some(condition),
            distribution: kaveon_sql::logical_plan::JoinDistribution::Partitioned,
        };
        let outer = |condition: Expr| LogicalPlan::Join {
            left: Box::new(inner(compare(
                column("c.c_custkey"),
                BinaryOp::Eq,
                column("o.o_custkey"),
            ))),
            right: Box::new(aliased("lineitem", "l")),
            join_type: JoinType::Inner,
            condition: Some(condition),
            distribution: kaveon_sql::logical_plan::JoinDistribution::Partitioned,
        };
        // The projection's columns reach the innermost scans, along with
        // every join key on the way.
        let pruned = push_projection_down(LogicalPlan::Project {
            input: Box::new(outer(compare(
                column("o.o_orderkey"),
                BinaryOp::Eq,
                column("l.l_orderkey"),
            ))),
            columns: vec![column("c.c_name"), column("l.l_quantity")],
        });
        let mut scans = Vec::new();
        scan_columns(&pruned, &mut scans);
        assert_eq!(
            scans,
            vec![
                &Some(vec!["c_custkey".to_owned(), "c_name".to_owned()]),
                &Some(vec!["o_custkey".to_owned(), "o_orderkey".to_owned()]),
                &Some(vec!["l_orderkey".to_owned(), "l_quantity".to_owned()]),
            ]
        );
        // Nothing above narrows the output: no scan is pruned, the join
        // keys notwithstanding, and neither does a filtered scan under
        // such a join lose everything but its predicate's columns.
        let whole = push_projection_down(LogicalPlan::Filter {
            input: Box::new(outer(compare(
                column("o.o_orderkey"),
                BinaryOp::Eq,
                column("l.l_orderkey"),
            ))),
            predicate: compare(column("l.l_quantity"), BinaryOp::Gt, int(1)),
        });
        let mut scans = Vec::new();
        scan_columns(&whole, &mut scans);
        assert_eq!(scans, vec![&None, &None, &None]);
        let filtered_side = push_projection_down(LogicalPlan::Sort {
            input: Box::new(LogicalPlan::Join {
                left: Box::new(LogicalPlan::Filter {
                    input: Box::new(aliased("customer", "c")),
                    predicate: compare(column("c_nationkey"), BinaryOp::Eq, int(1)),
                }),
                right: Box::new(aliased("orders", "o")),
                join_type: JoinType::Inner,
                condition: Some(compare(
                    column("c.c_custkey"),
                    BinaryOp::Eq,
                    column("o.o_custkey"),
                )),
                distribution: kaveon_sql::logical_plan::JoinDistribution::Partitioned,
            }),
            order_by: vec![(column("o.o_orderdate"), true)],
        });
        let mut scans = Vec::new();
        scan_columns(&filtered_side, &mut scans);
        assert_eq!(scans, vec![&None, &None]);
        // A semi join asked for everything keeps its left whole; asked for
        // named columns it adds its probe key. The subquery side prunes by
        // its own projection.
        let semi = |required: Option<Vec<&str>>| {
            let plan = LogicalPlan::SemiJoin {
                left: Box::new(aliased("orders", "o")),
                right: Box::new(LogicalPlan::Project {
                    input: Box::new(aliased("lineitem", "l")),
                    columns: vec![column("l_orderkey")],
                }),
                left_key: column("o_orderkey"),
                right_key: column("*"),
            };
            let plan = match required {
                Some(columns) => LogicalPlan::Project {
                    input: Box::new(plan),
                    columns: columns.into_iter().map(column).collect(),
                },
                None => plan,
            };
            let mut scans = Vec::new();
            let pruned = push_projection_down(plan);
            scan_columns(&pruned, &mut scans);
            scans.into_iter().cloned().collect::<Vec<_>>()
        };
        assert_eq!(semi(None), vec![None, Some(vec!["l_orderkey".to_owned()])]);
        assert_eq!(
            semi(Some(vec!["o_custkey"])),
            vec![
                Some(vec!["o_custkey".to_owned(), "o_orderkey".to_owned()]),
                Some(vec!["l_orderkey".to_owned()])
            ]
        );
        // A bare column the relation names do not describe keeps both
        // sides of that join whole.
        let bare = push_projection_down(LogicalPlan::Project {
            input: Box::new(outer(compare(
                column("o_orderkey"),
                BinaryOp::Eq,
                column("l.l_orderkey"),
            ))),
            columns: vec![column("c.c_name")],
        });
        let mut scans = Vec::new();
        scan_columns(&bare, &mut scans);
        assert_eq!(scans, vec![&None, &None, &None]);
    }

    #[test]
    fn semi_join_filters_move_into_the_left_input() {
        let plan = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::SemiJoin {
                left: Box::new(aliased("events", "t")),
                right: Box::new(aliased("users", "u")),
                left_key: column("country"),
                right_key: column("country"),
            }),
            predicate: compare(column("day"), BinaryOp::Eq, int(7)),
        };
        let LogicalPlan::SemiJoin { left, right, .. } = push_filter_down(plan) else {
            panic!("semi join stays the root");
        };
        assert!(filtered_scan(&left).is_some());
        assert!(matches!(*right, LogicalPlan::Scan { .. }));
    }
}
