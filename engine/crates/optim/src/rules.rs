use std::collections::{HashMap, HashSet};

use kaveon_core::{BinaryOp, CompareOp, Expr, ScalarValue, StoragePredicate};
use kaveon_sql::logical_plan::LogicalPlan;

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
        } => LogicalPlan::Join {
            left: Box::new(push_filter_down(*left)),
            right: Box::new(push_filter_down(*right)),
            join_type,
            condition,
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
        LogicalPlan::Filter { input, predicate } => {
            let mut columns = required.unwrap_or_default();
            collect_columns(&predicate, &mut columns);
            LogicalPlan::Filter {
                input: Box::new(prune_columns(*input, Some(columns))),
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
            let mut columns = required.unwrap_or_default();
            for (expression, _) in &order_by {
                collect_columns(expression, &mut columns);
            }
            LogicalPlan::Sort {
                input: Box::new(prune_columns(*input, Some(columns))),
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
            let mut columns = required.unwrap_or_default();
            for expr in &window_exprs {
                collect_columns(expr, &mut columns);
            }
            LogicalPlan::Window {
                input: Box::new(prune_columns(*input, Some(columns))),
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
        } => {
            let mut columns = required.unwrap_or_default();
            if let Some(condition) = &condition {
                collect_columns(condition, &mut columns);
            }
            let left_qualifier = plan_qualifier(&left);
            let right_qualifier = plan_qualifier(&right);
            let can_split = !columns.is_empty()
                && columns.iter().all(|column| column.contains('.'))
                && left_qualifier.is_some()
                && right_qualifier.is_some();
            let (left_required, right_required) = if can_split {
                let left_qualifier = left_qualifier.expect("qualifier checked");
                let right_qualifier = right_qualifier.expect("qualifier checked");
                let left_columns = columns
                    .iter()
                    .filter(|column| column.starts_with(&format!("{left_qualifier}.")))
                    .cloned()
                    .collect();
                let right_columns = columns
                    .iter()
                    .filter(|column| column.starts_with(&format!("{right_qualifier}.")))
                    .cloned()
                    .collect();
                (Some(left_columns), Some(right_columns))
            } else {
                (None, None)
            };
            LogicalPlan::Join {
                left: Box::new(prune_columns(*left, left_required)),
                right: Box::new(prune_columns(*right, right_required)),
                join_type,
                condition,
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

fn plan_qualifier(plan: &LogicalPlan) -> Option<String> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .clone()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned()),
        ),
        _ => None,
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
}
