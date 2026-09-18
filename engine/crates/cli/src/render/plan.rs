//! `EXPLAIN`: the query record's `plan.logical` as an indented tree.
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use serde_json::Value;

/// A `PlanNode` (`operator`, `attributes`, `children`) as a tree, one line
/// per node: the operator in the accent colour, its attributes dim as
/// `key=value`. `plan unavailable for this statement` when there is none.
pub fn tree(plan: &Value, theme: &Theme) -> Vec<Line<'static>> {
    if !plan.is_object() {
        return vec![Line::from(Span::styled(
            "  plan unavailable for this statement",
            theme.dim,
        ))];
    }
    let mut lines = Vec::new();
    node(plan, "  ", true, true, theme, &mut lines);
    lines
}

fn node(
    value: &Value,
    prefix: &str,
    root: bool,
    last: bool,
    theme: &Theme,
    lines: &mut Vec<Line<'static>>,
) {
    let operator = value
        .get("operator")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    let connector = if root {
        String::new()
    } else if last {
        "└─ ".to_owned()
    } else {
        "├─ ".to_owned()
    };
    let mut spans = vec![
        Span::raw(format!("{prefix}{connector}")),
        Span::styled(operator, theme.accent),
    ];
    if let Some(attributes) = value.get("attributes").and_then(Value::as_object) {
        for (key, attribute) in attributes {
            let shown = match attribute {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            spans.push(Span::styled(format!(" {key}={shown}"), theme.dim));
        }
    }
    lines.push(Line::from(spans));
    let children = value
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let child_prefix = if root {
        prefix.to_owned()
    } else if last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };
    for (index, child) in children.iter().enumerate() {
        node(
            child,
            &child_prefix,
            false,
            index + 1 == children.len(),
            theme,
            lines,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;
    use ratatui::style::{Color, Style};
    use serde_json::json;

    #[test]
    fn three_node_chain_is_indented_with_connectors() {
        let plan = json!({
            "id": 3, "phase": "logical", "operator": "Aggregate",
            "attributes": {"group": "[region]", "aggregates": "[COUNT(*)]"},
            "children": [{
                "id": 2, "phase": "logical", "operator": "Filter",
                "attributes": {"predicate": "year = 2025"},
                "children": [{
                    "id": 1, "phase": "logical", "operator": "Scan",
                    "attributes": {"table": "OpenSource.kaveon_product.kaveon_events", "rows": 18000000}
                }]
            }]
        });
        let text = to_plain(&tree(&plan, &Theme::mono()));
        assert_eq!(
            text,
            "  Aggregate aggregates=[COUNT(*)] group=[region]\n  \
             └─ Filter predicate=year = 2025\n     \
             └─ Scan rows=18000000 table=OpenSource.kaveon_product.kaveon_events\n"
        );
    }

    #[test]
    fn siblings_get_branch_connectors_and_rails() {
        let plan = json!({
            "operator": "Join",
            "attributes": {"kind": "inner"},
            "children": [
                {"operator": "Scan", "attributes": {"table": "a"},
                 "children": [{"operator": "Filter"}]},
                {"operator": "Scan", "attributes": {"table": "b"}}
            ]
        });
        let text = to_plain(&tree(&plan, &Theme::mono()));
        assert_eq!(
            text,
            "  Join kind=inner\n  ├─ Scan table=a\n  │  └─ Filter\n  └─ Scan table=b\n"
        );
    }

    #[test]
    fn operator_is_accented_and_attributes_dim() {
        let theme = Theme {
            accent: Style::default().fg(Color::Cyan),
            dim: Style::default().fg(Color::DarkGray),
            ..Theme::mono()
        };
        let plan = json!({"operator": "Scan", "attributes": {"table": "t"}});
        let lines = tree(&plan, &theme);
        let spans = &lines[0].spans;
        assert_eq!(spans[1].content, "Scan");
        assert_eq!(spans[1].style, theme.accent);
        assert_eq!(spans[2].content, " table=t");
        assert_eq!(spans[2].style, theme.dim);
    }

    #[test]
    fn missing_plan_says_so() {
        let text = to_plain(&tree(&Value::Null, &Theme::mono()));
        assert_eq!(text, "  plan unavailable for this statement\n");
    }
}
