//! `EXPLAIN`: the query record's plan as an indented tree. `EXPLAIN
//! ANALYZE`: the same over the optimized plan, then what the run cost —
//! the coordinator's phase timings and, per stage, every task with its
//! node, time, CPU, peak memory, scan and exchange volumes and spill.
use crate::client::session::{QueryRecord, Stage, Task};
use crate::render::table::{Cell, styled_cells};
use crate::render::{human_bytes, thousands};
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use serde_json::Value;

/// Which plan of the record `EXPLAIN` shows: the logical plan as written,
/// or the optimized one (pruned columns, pushed filters) that `ANALYZE`
/// pairs with the run's cost; the logical plan when the record has no
/// optimized one.
pub fn plan_of(record: &QueryRecord, optimized: bool) -> Value {
    let plans = record.plan.as_ref();
    let pick = |name: &str| {
        plans
            .and_then(|plan| plan.get(name))
            .filter(|v| v.is_object())
    };
    let chosen = if optimized {
        pick("optimized").or_else(|| pick("logical"))
    } else {
        pick("logical")
    };
    chosen.cloned().unwrap_or(Value::Null)
}

/// `EXPLAIN ANALYZE`: the run's cost under the plan. `width` fits the task
/// table to the terminal.
pub fn analyzed(record: &QueryRecord, width: Option<usize>, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::raw(""));
    let mode = record
        .execution
        .as_ref()
        .map(|execution| match &execution.detail {
            Some(detail) if !detail.is_empty() => format!("{} · {detail}", execution.mode),
            _ => execution.mode.clone(),
        })
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let mut header = vec![Span::styled("  Execution  ", theme.title), Span::raw(mode)];
    if let Some(cached) = &record.cached_from {
        header.push(Span::styled(
            format!(" · served from the result cache ({cached})"),
            theme.dim,
        ));
    }
    if let Some(timings) = &record.timings {
        let phases: Vec<String> = [
            ("analysis", timings.analysis_us),
            ("planning", timings.planning_us),
            ("execution", timings.execution_us),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|us| format!("{name} {}", micros(us))))
        .collect();
        if !phases.is_empty() {
            header.push(Span::styled(
                format!("   {}", phases.join(" · ")),
                theme.dim,
            ));
        }
    }
    lines.push(Line::from(header));
    if record.stages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no stages: the statement ran on the coordinator, or its record carries no tasks",
            theme.dim,
        )));
        return lines;
    }
    for stage in &record.stages {
        lines.push(Line::raw(""));
        lines.push(stage_line(stage, theme));
        let names: Vec<String> = [
            "task",
            "node",
            "elapsed",
            "cpu",
            "peak memory",
            "rows scanned",
            "bytes scanned",
            "rows out",
            "exchange in",
            "exchange out",
            "spilled",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let rows: Vec<Vec<Cell>> = stage
            .tasks
            .iter()
            .map(|task| task_cells(stage, task))
            .collect();
        let (table, _) = styled_cells(&names, &rows, width, theme);
        lines.extend(table.into_iter().map(indent));
    }
    lines
}

fn stage_line(stage: &Stage, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!("  Stage {}  ", stage.stage_id), theme.title),
        Span::raw(stage.state.clone()),
        Span::styled(
            format!(
                " · {}/{} tasks · {}",
                stage.completed_tasks,
                stage.task_count,
                micros(stage.elapsed_us)
            ),
            theme.dim,
        ),
    ];
    let nodes: std::collections::BTreeSet<&str> = stage
        .tasks
        .iter()
        .map(|task| task.node_id.as_str())
        .collect();
    if !nodes.is_empty() {
        spans.push(Span::styled(
            format!(
                " · {} {}",
                nodes.len(),
                if nodes.len() == 1 { "node" } else { "nodes" }
            ),
            theme.dim,
        ));
    }
    Line::from(spans)
}

fn task_cells(stage: &Stage, task: &Task) -> Vec<Cell> {
    let execution = task.execution.as_ref();
    let scan = task.scan.as_ref();
    let number = |value: u64| Cell::Number(thousands(i128::from(value)));
    let bytes = |value: u64| Cell::Number(human_bytes(value));
    let optional = |value: Option<u64>, zero_is_dash: bool| match value {
        Some(0) if zero_is_dash => Cell::Missing,
        Some(value) => number(value),
        None => Cell::Missing,
    };
    vec![
        Cell::plain(&format!("{}.{}", stage.stage_id, task.partition_index)),
        Cell::plain(&task.node_id),
        Cell::Number(micros(task.elapsed_us)),
        match execution {
            Some(execution) => Cell::Number(micros(execution.compute_cpu_us)),
            None => Cell::Missing,
        },
        match execution {
            Some(execution) => bytes(execution.memory_peak_bytes),
            None => Cell::Missing,
        },
        optional(scan.map(|scan| scan.rows_emitted), false),
        match scan {
            Some(scan) => bytes(scan.compressed_bytes_selected),
            None => Cell::Missing,
        },
        number(task.output_rows),
        match execution {
            Some(execution) => bytes(execution.exchange_input_bytes),
            None => Cell::Missing,
        },
        match execution {
            Some(execution) => bytes(execution.exchange_output_bytes),
            None => Cell::Missing,
        },
        match execution {
            Some(execution) if execution.spill_bytes_written > 0 => {
                bytes(execution.spill_bytes_written)
            }
            Some(_) => Cell::Missing,
            None => Cell::Missing,
        },
    ]
}

fn indent(mut line: Line<'static>) -> Line<'static> {
    line.spans.insert(0, Span::raw("  "));
    line
}

/// Microseconds as the summary spells durations: `357 ms`, `2.40 s`.
pub fn micros(us: u64) -> String {
    if us < 1_000 {
        format!("{us} µs")
    } else if us < 1_000_000 {
        format!("{} ms", us / 1_000)
    } else {
        format!("{:.2} s", us as f64 / 1_000_000.0)
    }
}

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
    use crate::client::session::QueryRecord;

    fn record() -> QueryRecord {
        serde_json::from_value(serde_json::json!({
            "id": "q1",
            "state": "FINISHED",
            "elapsed_ms": 357,
            "execution": {"mode": "distributed", "detail": "fragments"},
            "timings": {"analysis_us": 100, "planning_us": 121, "execution_us": 357383},
            "plan": {
                "logical": {"operator": "Aggregate", "attributes": {}, "children": [
                    {"operator": "Scan", "attributes": {"table": "t"}}]},
                "optimized": {"operator": "Aggregate", "attributes": {}, "children": [
                    {"operator": "Scan", "attributes": {"columns": "region", "table": "t"}}]}
            },
            "stages": [{
                "stage_id": 0, "state": "FINISHED", "task_count": 2, "completed_tasks": 2,
                "elapsed_us": 357373,
                "tasks": [
                    {"node_id": "worker-1", "partition_index": 0, "elapsed_us": 303692,
                     "output_rows": 6, "output_bytes": 1032,
                     "execution": {"compute_cpu_us": 145463, "compute_wall_us": 300554,
                                   "memory_peak_bytes": 1229922, "spill_bytes_written": 0,
                                   "exchange_input_bytes": 0, "exchange_output_bytes": 3472},
                     "scan": {"rows_selected": 3000000, "rows_emitted": 3000000,
                              "compressed_bytes_selected": 3812547, "read_ns": 203457218}},
                    {"node_id": "worker-2", "partition_index": 1, "elapsed_us": 22019,
                     "output_rows": 0, "output_bytes": 1032,
                     "execution": {"compute_cpu_us": 3089, "memory_peak_bytes": 0,
                                   "spill_bytes_written": 4096, "exchange_output_bytes": 2064}}
                ]
            }]
        }))
        .expect("a query record")
    }

    #[test]
    fn analyze_shows_timings_and_one_row_per_task() {
        let text = to_plain(&analyzed(&record(), Some(200), &Theme::mono()));
        assert!(
            text.contains("  Execution  distributed · fragments   analysis 100 µs · planning 121 µs · execution 357 ms"),
            "{text}"
        );
        assert!(
            text.contains("  Stage 0  FINISHED · 2/2 tasks · 357 ms · 2 nodes"),
            "{text}"
        );
        assert!(text.contains("│ task │ node     │ elapsed │    cpu │ peak memory │ rows scanned │ bytes scanned │ rows out │ exchange in │ exchange out │ spilled │"), "{text}");
        assert!(text.contains("│ 0.0  │ worker-1 │  303 ms │ 145 ms │     1.2 MiB │    3,000,000 │       3.6 MiB │        6 │         0 B │      3.4 KiB │ —       │"), "{text}");
        assert!(text.contains("│ 0.1  │ worker-2 │   22 ms │   3 ms │         0 B │ —            │ —             │        0 │         0 B │      2.0 KiB │ 4.0 KiB │"), "{text}");
    }

    #[test]
    fn analyze_without_stages_says_so_and_plan_of_falls_back() {
        let mut record = record();
        record.stages.clear();
        let text = to_plain(&analyzed(&record, None, &Theme::mono()));
        assert!(text.contains("no stages"), "{text}");
        assert_eq!(
            plan_of(&record, true)["children"][0]["attributes"]["columns"],
            serde_json::json!("region")
        );
        assert!(
            plan_of(&record, false)["children"][0]["attributes"]
                .get("columns")
                .is_none()
        );
        record.plan = None;
        assert!(plan_of(&record, true).is_null());
        assert_eq!(micros(999), "999 µs");
        assert_eq!(micros(2_400_000), "2.40 s");
    }

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
