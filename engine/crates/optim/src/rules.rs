use std::collections::HashMap;

use kaveon_core::{BinaryOp, CompareOp, Expr, ScalarValue, StoragePredicate};
use kaveon_sql::logical_plan::LogicalPlan;

/// Moves filters through semantics-preserving plan nodes toward their scans.
///
/// The filter is deliberately retained above the scan. Storage predicates only
/// eliminate row groups; they do not replace row-level predicate evaluation.
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
        scan @ LogicalPlan::Scan { .. } => scan,
    }
}

/// Converts an expression to the conservative predicate understood by storage.
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
        Expr::Column(_)
        | Expr::Literal(_)
        | Expr::Function { .. }
        | Expr::Star
        | Expr::Alias { .. } => None,
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
        Expr::Function { .. } | Expr::Star | Expr::Alias { .. } => None,
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
        | BinaryOp::Modulo => None,
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
}
