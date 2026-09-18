//! The prompt in front of the input and the one-line status bar under it.
use crate::theme::Theme;
use ratatui::text::{Line, Span};

pub struct StatusFacts<'a> {
    /// `catalog.schema`, once chosen.
    pub context: Option<(&'a str, &'a str)>,
    pub host: &'a str,
    pub workers_ready: Option<(usize, usize)>,
    pub last_elapsed_ms: Option<u64>,
    pub last_scanned_rows: Option<u64>,
    /// `NORMAL` / `INSERT` / `VISUAL` in vi editing; `None` in Emacs.
    pub mode: Option<&'static str>,
}

/// `catalog.schema · host · workers · last query …`; the context leads in
/// the accent colour, the rest is dim (warning colour when a worker is stale).
pub fn status_line(facts: &StatusFacts<'_>, theme: &Theme) -> Line<'static> {
    let mut parts = vec![facts.host.to_owned()];
    let mut style = theme.dim;
    match facts.workers_ready {
        Some((ready, 0)) => parts.push(format!(
            "{ready} worker{}",
            if ready == 1 { "" } else { "s" }
        )),
        Some((ready, stale)) => {
            parts.push(format!("{ready} workers · {stale} stale"));
            style = theme.warning;
        }
        None => {}
    }
    if let Some(ms) = facts.last_elapsed_ms {
        let mut last = format!("last query {:.2} s", ms as f64 / 1000.0);
        if let Some(rows) = facts.last_scanned_rows {
            last.push_str(&format!(
                ", {} rows scanned",
                crate::render::human_count(rows)
            ));
        }
        parts.push(last);
    }
    let mut spans = Vec::new();
    if let Some((catalog, schema)) = facts.context {
        spans.push(Span::styled(format!(" {catalog}"), theme.catalog));
        spans.push(Span::styled(".", theme.dim));
        spans.push(Span::styled(schema.to_owned(), theme.schema));
        spans.push(Span::styled(" ·", theme.dim));
    }
    spans.push(Span::styled(format!(" {}", parts.join(" · ")), style));
    if let Some(mode) = facts.mode {
        spans.push(Span::styled(format!(" · {mode}"), theme.accent));
    }
    Line::from(spans)
}

/// `kaveon › ` in front of the input; the context lives on the status line.
pub fn prompt(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(" kaveon", theme.title),
        Span::styled(" › ", theme.accent),
    ])
}

pub fn prompt_width() -> u16 {
    " kaveon › ".chars().count() as u16
}

pub fn host_of(server: &str) -> String {
    reqwest::Url::parse(server)
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            })
        })
        .unwrap_or_else(|| server.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_lists_context_host_workers_and_last_query() {
        let facts = StatusFacts {
            context: Some(("OpenSource", "kaveon_product")),
            host: "localhost:8081",
            workers_ready: Some((2, 0)),
            last_elapsed_ms: Some(1100),
            last_scanned_rows: Some(18_000_000),
            mode: None,
        };
        let line = status_line(&facts, &Theme::mono());
        assert_eq!(
            crate::render::to_plain(&[line]),
            " OpenSource.kaveon_product · localhost:8081 · 2 workers · last query 1.10 s, 18.0M rows scanned\n"
        );
        let bare = StatusFacts {
            context: None,
            host: "localhost:8081",
            workers_ready: None,
            last_elapsed_ms: None,
            last_scanned_rows: None,
            mode: Some("NORMAL"),
        };
        assert_eq!(
            crate::render::to_plain(&[status_line(&bare, &Theme::mono())]),
            " localhost:8081 · NORMAL\n"
        );
        assert_eq!(host_of("http://localhost:8081/"), "localhost:8081");
        assert_eq!(
            crate::render::to_plain(&[prompt(&Theme::mono())]),
            " kaveon › \n"
        );
        assert_eq!(prompt_width(), 10);
    }
}
