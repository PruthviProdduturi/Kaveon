//! Result-set labels for the Trino client. Native names also exist in saved
//! queries, so apply the protocol convention only at the result boundary.
use sqlparser::ast::{Expr, Select, SelectItem, SetExpr, Statement, TableFactor};

/// Final column positions for which SQL supplied no name. An expanded wildcard
/// preserves the names of its input columns.
pub fn anonymous_columns(sql: &str, width: usize) -> Vec<usize> {
    label_changes(sql, width)
        .into_iter()
        .filter_map(|(position, label)| matches!(label, LabelChange::Anonymous).then_some(position))
        .collect()
}

/// Positions whose native expression labels need Trino's output name.
/// Qualified references use their final identifier (`a.value` becomes
/// `value`); anonymous expressions are named `_colN` at the wire boundary.
pub fn column_label_changes(sql: &str, width: usize) -> Vec<(usize, Option<String>)> {
    label_changes(sql, width)
        .into_iter()
        .map(|(position, label)| {
            (
                position,
                match label {
                    LabelChange::Anonymous => None,
                    LabelChange::Named(name) => Some(name),
                },
            )
        })
        .collect()
}

enum LabelChange {
    Anonymous,
    Named(String),
}

fn label_changes(sql: &str, width: usize) -> Vec<(usize, LabelChange)> {
    let Ok(statements) = crate::parser::parse_sql(sql) else {
        return vec![];
    };
    let [Statement::Query(query)] = statements.as_slice() else {
        return vec![];
    };
    query_label_changes(&query.body, width)
}

fn query_label_changes(mut set: &SetExpr, width: usize) -> Vec<(usize, LabelChange)> {
    loop {
        match set {
            SetExpr::Query(query) => set = &query.body,
            SetExpr::SetOperation { left, .. } => set = left,
            SetExpr::Values(_) => {
                return (0..width)
                    .map(|position| (position, LabelChange::Anonymous))
                    .collect();
            }
            SetExpr::Select(select) => {
                if let Some(inner) = wildcard_subquery(select) {
                    set = inner;
                } else {
                    return select_label_changes(&select.projection, width);
                }
            }
            _ => return vec![],
        }
    }
}

/// A single unmodified star forwards a derived relation's fields positionally.
fn wildcard_subquery(select: &Select) -> Option<&SetExpr> {
    let [source] = select.from.as_slice() else {
        return None;
    };
    if !source.joins.is_empty() {
        return None;
    }
    let TableFactor::Derived {
        subquery, alias, ..
    } = &source.relation
    else {
        return None;
    };
    if alias
        .as_ref()
        .is_some_and(|alias| !alias.columns.is_empty())
    {
        return None;
    }
    match select.projection.as_slice() {
        [SelectItem::Wildcard(options)] | [SelectItem::QualifiedWildcard(_, options)]
            if *options == Default::default() =>
        {
            Some(&subquery.body)
        }
        _ => None,
    }
}

fn wildcard(item: &SelectItem) -> bool {
    matches!(
        item,
        SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..)
    )
}

fn unnamed(item: &SelectItem) -> bool {
    let SelectItem::UnnamedExpr(expression) = item else {
        return false;
    };
    let mut expr = expression;
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    !matches!(expr, Expr::Identifier(_) | Expr::CompoundIdentifier(_))
}

fn select_label_changes(items: &[SelectItem], width: usize) -> Vec<(usize, LabelChange)> {
    let stars: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| wildcard(item))
        .collect();
    let Some(star_columns) = width.checked_sub(items.len() - stars.len()) else {
        return vec![];
    };
    if stars.is_empty() {
        return if star_columns == 0 {
            items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| projection_label_change(item).map(|label| (i, label)))
                .collect()
        } else {
            vec![]
        };
    }
    let (first, first_star) = stars[0];
    let last = stars.last().unwrap().0;
    // Repeated identical wildcards expand to equal widths. For different
    // sources the native result schema gives only their combined width.
    let uniform = stars.iter().all(|(_, item)| *item == first_star)
        && star_columns.is_multiple_of(stars.len());
    let mut position = 0;
    let mut labels = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if wildcard(item) {
            if uniform {
                position += star_columns / stars.len();
            }
        } else {
            if let Some(label) = projection_label_change(item) {
                if uniform || i < first {
                    labels.push((position, label));
                } else if i > last {
                    labels.push((width - (items.len() - i), label));
                }
            }
            position += 1;
        }
    }
    labels
}

fn projection_label_change(item: &SelectItem) -> Option<LabelChange> {
    let SelectItem::UnnamedExpr(expression) = item else {
        return None;
    };
    let mut expression = expression;
    while let Expr::Nested(inner) = expression {
        expression = inner;
    }
    match expression {
        Expr::CompoundIdentifier(parts) if parts.len() > 1 => parts
            .last()
            .map(|identifier| LabelChange::Named(identifier.value.clone())),
        _ if unnamed(item) => Some(LabelChange::Anonymous),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{anonymous_columns, column_label_changes};

    #[test]
    fn labels_follow_the_projection_after_expanding_stars() {
        for (query, width, expected) in [
            (
                "SELECT (x), t.x, 1 AS expr_0, 2 AS \"_col3\", 3",
                5,
                vec![4],
            ),
            ("SELECT t.*, x + 2 FROM t", 4, vec![3]),
            ("SELECT 1, a.*, 2, a.*, 3 FROM a", 7, vec![0, 3, 6]),
            ("SELECT 1, a.*, b.*, 3 FROM a, b", 7, vec![0, 6]),
            ("SELECT 1 UNION ALL SELECT 2 AS x", 1, vec![0]),
            ("SELECT * FROM (SELECT 1 AS expr_0, 2) t", 2, vec![1]),
            ("SELECT t.* FROM (SELECT 1 AS expr_0, 2) t(x,y)", 2, vec![]),
        ] {
            assert_eq!(anonymous_columns(query, width), expected, "{query}");
        }
    }

    #[test]
    fn qualified_references_use_their_final_identifier_after_star_expansion() {
        for (query, width, expected) in [
            (
                "SELECT a.x, b.y FROM left_table a JOIN right_table b ON a.x = b.x",
                2,
                vec![(0, Some("x".to_owned())), (1, Some("y".to_owned()))],
            ),
            (
                "SELECT 1, a.*, b.y FROM left_table a JOIN right_table b ON a.x = b.x",
                4,
                vec![(0, None), (3, Some("y".to_owned()))],
            ),
            (
                "SELECT * FROM (SELECT a.x FROM left_table a) nested",
                1,
                vec![(0, Some("x".to_owned()))],
            ),
        ] {
            assert_eq!(column_label_changes(query, width), expected, "{query}");
        }
    }
}
