//! The two lines under every result: the verdict (time, rows, cost) and,
//! dimmed, the provenance (query id, placement, tasks, admission).
use crate::client::session::QueryRecord;
use crate::render::{human_bytes, human_count};
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub ok: bool,
    pub elapsed_ms: u64,
    pub rows: usize,
    /// What the rows are: "rows", "catalogs", "schemas", "tables", "columns".
    pub noun: &'static str,
    pub workers: usize,
    pub tasks: Option<(usize, usize)>,
    pub rows_scanned: Option<u64>,
    pub bytes_read: Option<u64>,
    pub partial_metrics: bool,
    pub query_id: Option<String>,
    pub placement: Option<String>,
    pub admission_wait_ms: u64,
    /// The statement was answered by the client over the catalog API.
    pub catalog_api: bool,
    pub cancelled: bool,
    pub message: Option<String>,
}

impl Summary {
    pub fn metadata(elapsed_ms: u64, rows: usize, noun: &'static str) -> Summary {
        Summary {
            ok: true,
            elapsed_ms,
            rows,
            noun,
            catalog_api: true,
            ..Summary::default()
        }
    }

    pub fn from_record(
        elapsed_ms: u64,
        rows: usize,
        query_id: &str,
        record: Option<&QueryRecord>,
    ) -> Summary {
        let mut summary = Summary {
            ok: true,
            elapsed_ms,
            rows,
            noun: "rows",
            query_id: Some(query_id.to_owned()),
            ..Summary::default()
        };
        let Some(record) = record else {
            return summary;
        };
        summary.workers = record
            .stages
            .iter()
            .flat_map(|stage| stage.tasks.iter().map(|task| task.node_id.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let total: usize = record.stages.iter().map(|stage| stage.task_count).sum();
        if total > 0 {
            let done: usize = record
                .stages
                .iter()
                .map(|stage| stage.completed_tasks)
                .sum();
            summary.tasks = Some((done, total));
        }
        if !record.scans.is_empty() {
            summary.rows_scanned = record
                .scans
                .iter()
                .map(|scan| scan.rows_emitted)
                .collect::<Option<Vec<_>>>()
                .map(|counts| counts.iter().sum());
            summary.bytes_read = Some(
                record
                    .scans
                    .iter()
                    .map(|scan| scan.compressed_bytes_selected)
                    .sum(),
            );
            summary.partial_metrics = record.scan_metrics_complete == Some(false);
        }
        summary.placement = record.execution.as_ref().map(|execution| {
            match (execution.mode.as_str(), execution.detail.as_deref()) {
                ("cache", _) => "from cache".to_owned(),
                ("coordinator", Some(reason)) => format!("on the coordinator: {reason}"),
                ("coordinator", None) => "on the coordinator".to_owned(),
                ("distributed", Some(path)) => format!("distributed ({path})"),
                (mode, _) => mode.to_owned(),
            }
        });
        summary.admission_wait_ms = record.admission_wait_ms;
        summary
    }

    fn seconds(&self) -> String {
        if self.elapsed_ms < 1000 {
            format!("{} ms", self.elapsed_ms)
        } else {
            format!("{:.2} s", self.elapsed_ms as f64 / 1000.0)
        }
    }

    fn verdict(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.cancelled {
            parts.push(format!("cancelled after {}", self.seconds()));
            return parts;
        }
        parts.push(self.seconds());
        parts.push(format!(
            "{} {}",
            crate::render::thousands(self.rows as i128),
            if self.rows == 1 {
                self.noun.trim_end_matches('s')
            } else {
                self.noun
            }
        ));
        if self.workers > 0 {
            parts.push(format!(
                "{} worker{}",
                self.workers,
                if self.workers == 1 { "" } else { "s" }
            ));
        }
        if let Some(scanned) = self.rows_scanned {
            let rate = if self.elapsed_ms > 0 {
                format!(
                    " at {} rows/s",
                    human_count((scanned as f64 * 1000.0 / self.elapsed_ms as f64) as u64)
                )
            } else {
                String::new()
            };
            parts.push(format!("{} rows scanned{rate}", human_count(scanned)));
        }
        if let Some(bytes) = self.bytes_read {
            parts.push(format!("{} read", human_bytes(bytes)));
        }
        if self.catalog_api {
            parts.push("catalog API".to_owned());
        }
        parts
    }

    fn provenance(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(id) = &self.query_id {
            parts.push(id.chars().take(8).collect());
        }
        if let Some(placement) = &self.placement {
            parts.push(placement.clone());
        }
        if let Some((done, total)) = self.tasks {
            parts.push(if done == total {
                format!("{total} task{}", if total == 1 { "" } else { "s" })
            } else {
                format!("{done}/{total} tasks")
            });
        }
        if self.admission_wait_ms > 0 {
            parts.push(format!(
                "waited {} ms for admission",
                self.admission_wait_ms
            ));
        }
        if self.partial_metrics {
            parts.push("partial worker metrics".to_owned());
        }
        if let Some(message) = &self.message {
            parts.push(message.clone());
        }
        parts
    }
}

pub fn lines(summary: &Summary, theme: &Theme) -> Vec<Line<'static>> {
    let (glyph, style) = if summary.ok && !summary.cancelled {
        ("✓", theme.ok)
    } else {
        ("✗", theme.error)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!(" {glyph} "), style),
        Span::raw(summary.verdict().join(" · ")),
    ])];
    let provenance = summary.provenance();
    if !provenance.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("   {}", provenance.join(" · ")),
            theme.dim,
        )));
    }
    lines
}

pub fn plain(summary: &Summary) -> String {
    crate::render::to_plain(&lines(summary, &Theme::mono()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_summary_reads_time_rows_cost_then_provenance() {
        let record: QueryRecord = serde_json::from_str(
            r#"{"id":"66aea874-dd02","state":"FINISHED","elapsed_ms":1096,"admission_wait_ms":12,
                "execution":{"mode":"distributed","detail":"fragments"},
                "stages":[{"task_count":5,"completed_tasks":5,"tasks":[{"node_id":"worker-1"},{"node_id":"worker-2"}]}],
                "scans":[{"rows_selected":18000000,"rows_emitted":18000000,"compressed_bytes_selected":6549472}]}"#,
        )
        .unwrap();
        let summary = Summary::from_record(1096, 5, "66aea874-dd02", Some(&record));
        assert_eq!(
            plain(&summary),
            " ✓ 1.10 s · 5 rows · 2 workers · 18.0M rows scanned at 16.4M rows/s · 6.2 MiB read\n   66aea874 · distributed (fragments) · 5 tasks · waited 12 ms for admission\n"
        );
    }

    #[test]
    fn metadata_summary_names_the_catalog_api() {
        assert_eq!(
            plain(&Summary::metadata(12, 2, "catalogs")),
            " ✓ 12 ms · 2 catalogs · catalog API\n"
        );
        assert_eq!(
            plain(&Summary::metadata(3, 1, "schemas")),
            " ✓ 3 ms · 1 schema · catalog API\n"
        );
    }

    #[test]
    fn cache_hits_and_cancellations_are_visible() {
        let record: QueryRecord = serde_json::from_str(
            r#"{"id":"q","state":"FINISHED","elapsed_ms":2,"execution":{"mode":"cache","detail":"hit"}}"#,
        )
        .unwrap();
        let summary = Summary::from_record(2, 6, "q", Some(&record));
        assert!(plain(&summary).contains("   q · from cache\n"));
        let cancelled = Summary {
            ok: false,
            cancelled: true,
            elapsed_ms: 4100,
            ..Summary::default()
        };
        assert_eq!(plain(&cancelled), " ✗ cancelled after 4.10 s\n");
    }
}
