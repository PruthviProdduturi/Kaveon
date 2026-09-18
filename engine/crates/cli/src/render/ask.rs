//! What `.ask` prints for each shape of answer: the dataset and title
//! dimmed, then the SQL or the rows, then the provenance.
use crate::client::dlm::AskAnswer;
use crate::render::table;
use crate::shell::highlight::highlight;
use crate::theme::Theme;
use ratatui::text::{Line, Span};

/// The lines for an answer. For `Live` the SQL is the body: the caller runs
/// it on the coordinator when the answer says `engine` and prints the result
/// after these lines.
pub fn answer_lines(answer: &AskAnswer, theme: &Theme) -> Vec<Line<'static>> {
    match answer {
        AskAnswer::Live {
            dataset,
            sql,
            engine,
            title,
            note,
            confidence,
            ..
        } => {
            let mut lines = vec![header(dataset, title.as_deref(), theme)];
            lines.extend(highlight(sql.trim_end(), theme));
            lines.extend(provenance(note.as_deref(), *confidence, theme));
            if !*engine {
                lines.push(dim(
                    "this dataset is served by the platform API, not the coordinator — run the SQL in SQL Lab",
                    theme,
                ));
            }
            lines
        }
        AskAnswer::Context {
            dataset,
            columns,
            rows,
            title,
            note,
            approx,
            confidence,
            ..
        } => {
            let mut lines = vec![header(dataset, title.as_deref(), theme)];
            lines.extend(table::styled(columns, rows, None, theme).0);
            lines.push(dim(
                if *approx {
                    "from context · no scan · ≈ approximate"
                } else {
                    "from context · no scan"
                },
                theme,
            ));
            lines.extend(provenance(note.as_deref(), *confidence, theme));
            lines
        }
        AskAnswer::Clarify {
            dataset,
            prompt,
            options,
            ..
        } => {
            let mut lines = Vec::with_capacity(options.len() + 3);
            if let Some(dataset) = dataset {
                lines.push(dim(&format!("→ {dataset}"), theme));
            }
            lines.push(Line::raw(prompt.clone()));
            let width = options.len().to_string().len();
            for (index, (_, label, description)) in options.iter().enumerate() {
                let mut spans = vec![
                    Span::styled(format!("{:>width$}  ", index + 1), theme.accent),
                    Span::raw(label.clone()),
                ];
                if !description.is_empty() {
                    spans.push(Span::styled(format!(" — {description}"), theme.dim));
                }
                lines.push(Line::from(spans));
            }
            lines.push(dim("reply with .ask <number> to choose", theme));
            lines
        }
        AskAnswer::OutOfScope { datasets, hint } => {
            let mut lines = vec![Line::raw("not a question about a registered dataset")];
            if !datasets.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("datasets  ", theme.dim),
                    Span::raw(datasets.join(", ")),
                ]));
            }
            if let Some(hint) = hint {
                lines.push(dim(hint, theme));
            }
            lines
        }
        AskAnswer::Refused { reason } => vec![Line::raw(refusal(reason))],
    }
}

fn header(dataset: &str, title: Option<&str>, theme: &Theme) -> Line<'static> {
    let text = match title {
        Some(title) if !title.is_empty() => format!("→ {dataset} · {title}"),
        _ => format!("→ {dataset}"),
    };
    dim(&text, theme)
}

fn provenance(note: Option<&str>, confidence: Option<f64>, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(2);
    if let Some(note) = note
        && !note.is_empty()
    {
        lines.push(dim(note, theme));
    }
    if let Some(confidence) = confidence {
        lines.push(dim(&format!("confidence {confidence:.2}"), theme));
    }
    lines
}

fn dim(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(text.to_owned(), theme.dim))
}

/// The API's refusal codes in plain words.
fn refusal(reason: &str) -> String {
    match reason {
        "no_dataset" => {
            "the question mentions known terms, but no registered dataset answers it".to_owned()
        }
        "dataset_not_found" => "the dataset this question routes to no longer exists".to_owned(),
        "no_fact_table" => "the dataset has no fact table to answer from".to_owned(),
        "out_of_scope" => "not a question about a registered dataset".to_owned(),
        other => format!("the platform declined the question ({other})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;
    use serde_json::json;

    fn plain(answer: &AskAnswer) -> String {
        to_plain(&answer_lines(answer, &Theme::mono()))
    }

    #[test]
    fn live_answer_shows_the_sql_then_the_provenance() {
        let answer = AskAnswer::Live {
            dataset: "Sales".into(),
            catalog: "kaveon".into(),
            schema: "sales".into(),
            sql: "SELECT region, SUM(net) AS net\nFROM orders\nGROUP BY 1\n".into(),
            engine: true,
            title: Some("Net revenue by region".into()),
            note: Some("latest available year".into()),
            confidence: Some(0.834),
            frame: None,
            duration_ms: Some(12.0),
        };
        assert_eq!(
            plain(&answer),
            "→ Sales · Net revenue by region\nSELECT region, SUM(net) AS net\nFROM orders\nGROUP BY 1\nlatest available year\nconfidence 0.83\n"
        );
    }

    #[test]
    fn live_answer_off_the_engine_ends_with_the_sql_lab_note() {
        let answer = AskAnswer::Live {
            dataset: "Sales".into(),
            catalog: "azuresql".into(),
            schema: "dbo".into(),
            sql: "SELECT COUNT(*) FROM orders".into(),
            engine: false,
            title: None,
            note: None,
            confidence: None,
            frame: None,
            duration_ms: None,
        };
        assert_eq!(
            plain(&answer),
            "→ Sales\nSELECT COUNT(*) FROM orders\nthis dataset is served by the platform API, not the coordinator — run the SQL in SQL Lab\n"
        );
    }

    #[test]
    fn context_answer_tabulates_the_rows_and_marks_approximation() {
        let answer = AskAnswer::Context {
            dataset: "Sales".into(),
            columns: vec!["region".into(), "net".into()],
            rows: vec![
                vec![json!("East"), json!(1200)],
                vec![json!("West"), json!(null)],
            ],
            title: Some("net by region".into()),
            note: None,
            approx: false,
            confidence: Some(0.9),
            frame: None,
            duration_ms: None,
        };
        let text = plain(&answer);
        assert!(text.starts_with("→ Sales · net by region\n┌"), "{text}");
        assert!(text.contains("│ East   │ 1,200 │"), "{text}");
        assert!(text.contains("│ West   │ NULL  │"), "{text}");
        assert!(
            text.ends_with("┘\nfrom context · no scan\nconfidence 0.90\n"),
            "{text}"
        );
        let approx = AskAnswer::Context {
            dataset: "Sales".into(),
            columns: vec!["region".into(), "net".into()],
            rows: vec![vec![json!("East"), json!(1200)]],
            title: Some("net by region".into()),
            note: Some("≈ estimated from a HyperLogLog sketch".into()),
            approx: true,
            confidence: Some(0.9),
            frame: None,
            duration_ms: None,
        };
        let text = plain(&approx);
        assert!(
            text.ends_with(
                "┘\nfrom context · no scan · ≈ approximate\n≈ estimated from a HyperLogLog sketch\nconfidence 0.90\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn clarification_numbers_the_options() {
        let answer = AskAnswer::Clarify {
            dataset: Some("Sales".into()),
            prompt: "Which revenue?".into(),
            kind: "metric".into(),
            options: vec![
                ("net".into(), "Net revenue".into(), "after returns".into()),
                ("gross".into(), "Gross revenue".into(), String::new()),
            ],
            frame: None,
            resume: crate::client::dlm::Resume::default(),
        };
        assert_eq!(
            plain(&answer),
            "→ Sales\nWhich revenue?\n1  Net revenue — after returns\n2  Gross revenue\nreply with .ask <number> to choose\n"
        );
    }

    #[test]
    fn out_of_scope_lists_the_datasets_and_the_hint() {
        let answer = AskAnswer::OutOfScope {
            datasets: vec!["Sales".into(), "Energy".into()],
            hint: Some("That is SQL. Run it in SQL Lab.".into()),
        };
        assert_eq!(
            plain(&answer),
            "not a question about a registered dataset\ndatasets  Sales, Energy\nThat is SQL. Run it in SQL Lab.\n"
        );
        let bare = AskAnswer::OutOfScope {
            datasets: vec![],
            hint: None,
        };
        assert_eq!(plain(&bare), "not a question about a registered dataset\n");
    }

    #[test]
    fn refusals_read_as_sentences() {
        for (reason, expected) in [
            (
                "no_dataset",
                "the question mentions known terms, but no registered dataset answers it",
            ),
            (
                "dataset_not_found",
                "the dataset this question routes to no longer exists",
            ),
            (
                "no_fact_table",
                "the dataset has no fact table to answer from",
            ),
            (
                "throttled",
                "the platform declined the question (throttled)",
            ),
        ] {
            let answer = AskAnswer::Refused {
                reason: reason.into(),
            };
            assert_eq!(plain(&answer), format!("{expected}\n"));
        }
    }
}
