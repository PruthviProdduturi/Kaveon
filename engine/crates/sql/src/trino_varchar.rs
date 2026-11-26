//! Trino VARCHAR bounds are part of the SQL result schema. Arrow string
//! arrays store the values but carry no declared maximum character count.
use crate::logical_plan::{AggregateExpr, LogicalPlan};
use kaveon_core::{Expr, predicate::ScalarValue};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    Null,
    Bounded(u32),
    Unknown,
}

impl Width {
    fn common(self, other: Self) -> Self {
        match (self, other) {
            (Self::Null, w) | (w, Self::Null) => w,
            (Self::Bounded(a), Self::Bounded(b)) => Self::Bounded(a.max(b)),
            _ => Self::Unknown,
        }
    }
}

type Field = (String, Width);

fn name(expr: &Expr) -> String {
    match expr {
        Expr::Column(name) | Expr::Alias { name, .. } => name.clone(),
        _ => String::new(),
    }
}

fn expression(expr: &Expr, input: &[Field]) -> Width {
    match expr {
        Expr::Literal(ScalarValue::Utf8(s)) => Width::Bounded(s.chars().count() as u32),
        Expr::Literal(ScalarValue::Null) => Width::Null,
        Expr::Column(key) => {
            let mut fields = input.iter().filter(|(name, _)| {
                name.eq_ignore_ascii_case(key)
                    || (!key.contains('.')
                        && name
                            .rsplit('.')
                            .next()
                            .is_some_and(|s| s.eq_ignore_ascii_case(key)))
            });
            match (fields.next(), fields.next()) {
                (Some((_, w)), None) => *w,
                _ => Width::Unknown,
            }
        }
        Expr::Alias { expr, .. } => expression(expr, input),
        Expr::Case {
            when_then,
            else_expr,
            ..
        } => {
            let default = else_expr
                .as_deref()
                .map_or(Width::Null, |e| expression(e, input));
            when_then
                .iter()
                .fold(default, |w, (_, e)| w.common(expression(e, input)))
        }
        Expr::Function { name, args } => {
            if let Some(width) = name
                .strip_prefix("KAVEON_CAST_TEXT_")
                .and_then(|s| s.strip_suffix("_VARCHAR"))
                .and_then(|s| s.parse().ok())
            {
                return Width::Bounded(width);
            }
            match (name.as_str(), args.as_slice()) {
                ("REPLACE", [s, _]) => expression(s, input),
                ("REPLACE", [s, _, replacement]) => {
                    match (expression(s, input), expression(replacement, input)) {
                        (Width::Bounded(x), Width::Bounded(z)) => {
                            Width::Bounded(x.saturating_add(z.saturating_mul(x.saturating_add(1))))
                        }
                        _ => Width::Unknown,
                    }
                }
                ("COALESCE", args) => args
                    .iter()
                    .fold(Width::Null, |w, e| w.common(expression(e, input))),
                _ => Width::Unknown,
            }
        }
        _ => Width::Unknown,
    }
}

fn merge(mut left: Vec<Field>, right: Vec<Field>) -> Option<Vec<Field>> {
    if left.len() != right.len() {
        return None;
    }
    for ((_, l), (_, r)) in left.iter_mut().zip(right) {
        *l = l.common(r);
    }
    Some(left)
}

fn schema(plan: &LogicalPlan) -> Option<Vec<Field>> {
    use LogicalPlan::*;
    match plan {
        Scan { columns, .. } => Some(
            columns
                .as_ref()?
                .iter()
                .map(|n| (n.clone(), Width::Unknown))
                .collect(),
        ),
        Unnest {
            expressions,
            columns,
            with_ordinality,
            ordinality_column,
            ..
        } => {
            if expressions.len() != columns.len() {
                return None;
            }
            let mut fields: Vec<_> = columns
                .iter()
                .cloned()
                .zip(expressions.iter().map(|e| {
                    match e {
                        Expr::Function { name, args } if name == "ARRAY" => args
                            .iter()
                            .fold(Width::Null, |w, e| w.common(expression(e, &[]))),
                        _ => Width::Unknown,
                    }
                }))
                .collect();
            if *with_ordinality {
                fields.push((ordinality_column.clone(), Width::Unknown));
            }
            Some(fields)
        }
        Project { input, columns } => {
            let input = schema(input);
            let mut fields = Vec::new();
            for e in columns {
                if matches!(e, Expr::Star) {
                    fields.extend(input.as_ref()?.iter().cloned());
                } else {
                    fields.push((name(e), expression(e, input.as_deref().unwrap_or_default())));
                }
            }
            Some(fields)
        }
        Aggregate {
            input,
            group_by,
            aggregates,
        } => {
            let input = schema(input).unwrap_or_default();
            Some(
                group_by
                    .iter()
                    .map(|e| (name(e), expression(e, &input)))
                    .chain(aggregates.iter().map(|a| {
                        let width = match a {
                            AggregateExpr::Min(e) | AggregateExpr::Max(e) => expression(e, &input),
                            _ => Width::Unknown,
                        };
                        (a.output_name(), width)
                    }))
                    .collect(),
            )
        }
        Join {
            left,
            right,
            using_columns,
            ..
        } if using_columns.is_none() => {
            let mut fields = schema(left)?;
            fields.extend(schema(right)?);
            Some(fields)
        }
        Filter { input, .. }
        | Sort { input, .. }
        | Offset { input, .. }
        | Limit { input, .. }
        | Distinct { input }
        | SemiJoin { left: input, .. }
        | AntiJoin { left: input, .. } => schema(input),
        Union { inputs, .. } => {
            let (first, rest) = inputs.split_first()?;
            rest.iter()
                .try_fold(schema(first)?, |l, r| merge(l, schema(r)?))
        }
        Except { left, right, .. } | Intersect { left, right, .. } => {
            merge(schema(left)?, schema(right)?)
        }
        Window {
            input,
            window_exprs,
        } => {
            let mut fields = schema(input)?;
            fields.extend(window_exprs.iter().map(|e| (name(e), Width::Unknown)));
            Some(fields)
        }
        Join { .. } => None,
    }
}

/// Ordinal bounds known from SQL types, including empty inputs and typed NULL.
pub fn output_bounds(plan: &LogicalPlan) -> Vec<Option<u32>> {
    schema(plan)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, w)| match w {
            Width::Bounded(n) if n < i32::MAX as u32 => Some(n),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::output_bounds;
    use crate::logical_plan::trino_logical_plan;

    #[test]
    fn declared_width_follows_the_relation_and_counts_unicode_scalars() {
        for (sql, expected) in [
            ("SELECT replace('café','é','ø')", vec![Some(9)]),
            (
                "SELECT CAST(NULL AS VARCHAR(7)), CAST('x' AS VARCHAR), ''",
                vec![Some(7), None, Some(0)],
            ),
            (
                "SELECT t.s FROM (VALUES ('é漢🙂'),('a'),(NULL)) t(s) WHERE 1=0",
                vec![Some(3)],
            ),
            (
                "SELECT CAST('a' AS VARCHAR(5)) UNION ALL SELECT CAST('b' AS VARCHAR(8))",
                vec![Some(8)],
            ),
            (
                "SELECT CAST('a' AS VARCHAR(5)) UNION ALL SELECT CAST('b' AS VARCHAR)",
                vec![None],
            ),
            (
                "SELECT k, COUNT(*), SUM(v) FROM (VALUES ('a',1),('a',NULL),('b',2)) t(k,v) GROUP BY k ORDER BY k",
                vec![Some(1), None, None],
            ),
            (
                "SELECT a.x,b.y FROM (VALUES (1),(2)) a(x) JOIN (VALUES (2,'p'),(2,'qr')) b(k,y) ON a.x=b.k",
                vec![None, Some(2)],
            ),
        ] {
            let plan = trino_logical_plan(sql, &[], &Default::default(), false).unwrap();
            assert_eq!(output_bounds(&plan), expected, "{sql}: {plan:?}");
        }
    }
}
