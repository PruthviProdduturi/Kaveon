//! The running line above the editor while a statement is on the
//! coordinator: spinner, phase, elapsed time and whatever counters the
//! query record reports.
use crate::client::session::QueryRecord;
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;
use std::time::Duration;

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// The hint starts here when the line is shorter; two spaces at least.
const HINT_COLUMN: usize = 72;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Posted, record not found yet.
    #[default]
    Submitting,
    /// Waiting for memory admission.
    Queued,
    Running,
    /// Ctrl-C sent the cancel; waiting for the coordinator to let go.
    Cancelling,
}

#[derive(Default, Clone, Debug)]
pub struct Progress {
    pub phase: Phase,
    pub elapsed: Duration,
    pub query_id: Option<String>,
    pub admission_wait_ms: u64,
    /// Statements queued ahead of this one, from the cluster payload.
    pub queue_ahead: Option<u64>,
    pub tasks_done: usize,
    pub tasks_total: usize,
    pub workers: usize,
    pub rows_scanned: u64,
    /// Result rows the coordinator has written so far, for a paged
    /// statement whose pages are being read while it runs.
    pub rows_so_far: Option<usize>,
}

impl Progress {
    pub fn from_record(record: &QueryRecord, elapsed: Duration) -> Progress {
        let phase = match record.state.as_str() {
            "QUEUED" => Phase::Queued,
            _ => Phase::Running,
        };
        let workers = record
            .stages
            .iter()
            .flat_map(|stage| stage.tasks.iter().map(|task| task.node_id.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        Progress {
            phase,
            elapsed,
            query_id: Some(record.id.clone()),
            admission_wait_ms: record.admission_wait_ms,
            queue_ahead: None,
            tasks_done: record
                .stages
                .iter()
                .map(|stage| stage.completed_tasks)
                .sum(),
            tasks_total: record.stages.iter().map(|stage| stage.task_count).sum(),
            workers,
            rows_scanned: record
                .scans
                .iter()
                .filter_map(|scan| scan.rows_emitted)
                .sum(),
            rows_so_far: None,
        }
    }
}

pub fn line(progress: &Progress, tick: usize, theme: &Theme) -> Line<'static> {
    let seconds = format!("{:.1} s", progress.elapsed.as_secs_f64());
    let mut parts = vec![match progress.phase {
        Phase::Submitting => format!("Submitting {seconds}"),
        Phase::Queued => {
            let mut text = format!("Queued {seconds} for memory admission");
            if let Some(ahead) = progress.queue_ahead {
                text.push_str(&format!(" · {ahead} ahead"));
            }
            text
        }
        Phase::Running => format!("Running {seconds}"),
        Phase::Cancelling => format!("Cancelling {seconds}"),
    }];
    if matches!(progress.phase, Phase::Running | Phase::Cancelling) {
        if progress.tasks_total > 0 {
            parts.push(format!(
                "{}/{} tasks",
                progress.tasks_done, progress.tasks_total
            ));
        }
        if progress.workers > 0 {
            parts.push(format!(
                "{} worker{}",
                progress.workers,
                if progress.workers == 1 { "" } else { "s" }
            ));
        }
        if progress.rows_scanned > 0 {
            parts.push(format!(
                "{} rows scanned",
                crate::render::human_count(progress.rows_scanned)
            ));
        }
        if progress.admission_wait_ms > 0 {
            parts.push(format!(
                "waited {} ms for admission",
                progress.admission_wait_ms
            ));
        }
        if let Some(rows) = progress.rows_so_far {
            parts.push(format!(
                "{} rows so far",
                crate::render::thousands(rows as i128)
            ));
        }
    }
    let body = format!(" {} {}", SPINNER[tick % SPINNER.len()], parts.join(" · "));
    let mut spans = vec![Span::styled(body.clone(), theme.accent)];
    if !matches!(progress.phase, Phase::Cancelling) {
        let padding = HINT_COLUMN.saturating_sub(body.chars().count()).max(2);
        spans.push(Span::styled(
            format!("{}Ctrl-C to cancel", " ".repeat(padding)),
            theme.dim,
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;

    #[test]
    fn running_line_reports_state_and_counters_only_when_known() {
        let theme = Theme::mono();
        let submitting = Progress {
            phase: Phase::Submitting,
            elapsed: Duration::from_millis(400),
            ..Progress::default()
        };
        let text = to_plain(&[line(&submitting, 0, &theme)]);
        assert!(text.starts_with(" ⠋ Submitting 0.4 s"), "{text}");
        assert!(text.ends_with("Ctrl-C to cancel\n"), "{text}");

        let queued = Progress {
            phase: Phase::Queued,
            elapsed: Duration::from_millis(3200),
            queue_ahead: Some(2),
            ..Progress::default()
        };
        let text = to_plain(&[line(&queued, 1, &theme)]);
        assert!(
            text.contains("⠙ Queued 3.2 s for memory admission · 2 ahead"),
            "{text}"
        );

        let running = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_millis(2400),
            tasks_done: 3,
            tasks_total: 5,
            workers: 2,
            rows_scanned: 210_000_000,
            ..Progress::default()
        };
        let text = to_plain(&[line(&running, 2, &theme)]);
        assert!(
            text.contains("Running 2.4 s · 3/5 tasks · 2 workers · 210.0M rows scanned"),
            "{text}"
        );
        assert!(text.ends_with("Ctrl-C to cancel\n"), "{text}");

        let bare = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_secs(1),
            workers: 2,
            ..Progress::default()
        };
        let text = to_plain(&[line(&bare, 0, &theme)]);
        assert!(text.contains("Running 1.0 s · 2 workers"), "{text}");
        assert!(
            !text.contains("tasks") && !text.contains("scanned"),
            "{text}"
        );

        let one = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_secs(1),
            workers: 1,
            ..Progress::default()
        };
        let text = to_plain(&[line(&one, 0, &theme)]);
        assert!(
            text.contains("· 1 worker ") && !text.contains("workers"),
            "{text}"
        );

        let admitted_late = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_secs(5),
            workers: 2,
            admission_wait_ms: 3000,
            ..Progress::default()
        };
        let text = to_plain(&[line(&admitted_late, 0, &theme)]);
        assert!(
            text.contains("Running 5.0 s · 2 workers · waited 3000 ms for admission"),
            "{text}"
        );

        let cancelling = Progress {
            phase: Phase::Cancelling,
            elapsed: Duration::from_secs(4),
            ..Progress::default()
        };
        let text = to_plain(&[line(&cancelling, 0, &theme)]);
        assert!(text.contains("Cancelling 4.0 s"), "{text}");
        assert!(!text.contains("Ctrl-C"), "{text}");
    }

    #[test]
    fn rows_written_so_far_close_the_running_line() {
        let theme = Theme::mono();
        let streaming = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_millis(1800),
            workers: 2,
            rows_scanned: 12_000_000,
            rows_so_far: Some(12_000),
            ..Progress::default()
        };
        let text = to_plain(&[line(&streaming, 0, &theme)]);
        assert!(
            text.contains("Running 1.8 s · 2 workers · 12.0M rows scanned · 12,000 rows so far"),
            "{text}"
        );
        let nothing_yet = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_millis(300),
            rows_so_far: Some(0),
            ..Progress::default()
        };
        let text = to_plain(&[line(&nothing_yet, 0, &theme)]);
        assert!(text.contains("Running 0.3 s · 0 rows so far"), "{text}");
        let inline = Progress {
            phase: Phase::Running,
            elapsed: Duration::from_millis(300),
            ..Progress::default()
        };
        let text = to_plain(&[line(&inline, 0, &theme)]);
        assert!(!text.contains("so far"), "{text}");
        let record: QueryRecord = serde_json::from_str(r#"{"id":"q","state":"RUNNING"}"#).unwrap();
        assert_eq!(
            Progress::from_record(&record, Duration::ZERO).rows_so_far,
            None
        );
    }

    #[test]
    fn spinner_cycles_through_its_frames() {
        let theme = Theme::mono();
        let progress = Progress::default();
        let first = to_plain(&[line(&progress, 0, &theme)]);
        let wrapped = to_plain(&[line(&progress, SPINNER.len(), &theme)]);
        let next = to_plain(&[line(&progress, 1, &theme)]);
        assert_eq!(first, wrapped);
        assert_ne!(first, next);
    }

    #[test]
    fn progress_from_record_reads_state_and_stage_totals() {
        let record: QueryRecord = serde_json::from_str(
            r#"{"id":"q","state":"RUNNING","admission_wait_ms":120,"stages":[{"task_count":4,"completed_tasks":1,"tasks":[{"node_id":"a"},{"node_id":"b"}]},{"task_count":1,"completed_tasks":0,"tasks":[{"node_id":"a"}]}],"scans":[{"rows_selected":10,"rows_emitted":7,"compressed_bytes_selected":1}]}"#,
        )
        .unwrap();
        let progress = Progress::from_record(&record, Duration::from_secs(1));
        assert!(matches!(progress.phase, Phase::Running));
        assert_eq!(
            (
                progress.tasks_done,
                progress.tasks_total,
                progress.workers,
                progress.rows_scanned
            ),
            (1, 5, 2, 7)
        );
        assert_eq!(progress.query_id.as_deref(), Some("q"));
        assert_eq!(progress.admission_wait_ms, 120);
        assert_eq!(progress.elapsed, Duration::from_secs(1));

        let queued: QueryRecord = serde_json::from_str(r#"{"id":"q2","state":"QUEUED"}"#).unwrap();
        let progress = Progress::from_record(&queued, Duration::from_millis(10));
        assert!(matches!(progress.phase, Phase::Queued));
        assert_eq!((progress.tasks_total, progress.workers), (0, 0));
    }
}
