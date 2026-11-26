//! Give integer literals their Trino type in value projections and arithmetic.
//! Structural numbers such as ORDER BY ordinals and LIMIT must stay literals.
use std::ops::ControlFlow;

use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, Expr, Query, SelectItem, SetExpr, Statement, UnaryOperator,
    Value, VisitMut, VisitorMut,
};

pub(crate) fn bind(statements: &mut [Statement]) {
    for statement in statements {
        let _ = statement.visit(&mut IntegerLiterals);
    }
}

struct IntegerLiterals;

impl VisitorMut for IntegerLiterals {
    type Break = ();

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<()> {
        if let Expr::BinaryOp { left, op, right } = expr
            && matches!(
                op,
                BinaryOperator::Plus
                    | BinaryOperator::Minus
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo
            )
        {
            type_literal(left);
            type_literal(right);
        }
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, query: &mut Query) -> ControlFlow<()> {
        type_outputs(&mut query.body);
        ControlFlow::Continue(())
    }
}

fn type_outputs(set: &mut SetExpr) {
    match set {
        SetExpr::Select(select) => {
            for item in &mut select.projection {
                if let SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } = item
                {
                    type_literal(expr);
                }
            }
        }
        SetExpr::Values(values) => values.rows.iter_mut().flatten().for_each(type_literal),
        SetExpr::SetOperation { left, right, .. } => {
            type_outputs(left);
            type_outputs(right);
        }
        _ => {}
    }
}

fn signed_literal(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Value(Value::Number(value, _)) => value.parse().ok(),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => signed_literal(expr)?.checked_neg(),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        }
        | Expr::Nested(expr) => signed_literal(expr),
        _ => None,
    }
}

fn type_literal(expr: &mut Expr) {
    if let Some(value) = signed_literal(expr).and_then(|v| i32::try_from(v).ok()) {
        *expr = Expr::Cast {
            kind: CastKind::Cast,
            expr: Box::new(Expr::Value(Value::Number(value.to_string(), false))),
            data_type: DataType::Int(None),
            format: None,
        };
    }
}
