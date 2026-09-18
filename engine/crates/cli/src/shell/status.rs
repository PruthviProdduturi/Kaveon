//! The editor box title and the one-line status bar under it.
use crate::theme::Theme;
use ratatui::text::{Line, Span};

pub struct StatusFacts<'a> {
    pub host: &'a str,
    pub workers_ready: Option<(usize, usize)>,
    pub last_elapsed_ms: Option<u64>,
    pub last_scanned_rows: Option<u64>,
}

pub fn box_title(catalog: &str, schema: &str) -> String {
    format!("{catalog}.{schema}")
}

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
    Line::from(Span::styled(format!(" {}", parts.join(" · ")), style))
}

/// `context › ` in front of the input.
/// `KAVEON: catalog.schema › ` — the catalog plain, the schema dimmed so the
/// two read apart — or just `KAVEON › ` until a context is chosen.
pub fn prompt(context: Option<(&str, &str)>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(" KAVEON", theme.title)];
    match context {
        Some((catalog, schema)) => {
            spans.push(Span::raw(format!(": {catalog}")));
            spans.push(Span::styled(format!(".{schema} "), theme.dim));
        }
        None => spans.push(Span::raw(" ")),
    }
    spans.push(Span::styled("› ", theme.accent));
    Line::from(spans)
}

pub fn prompt_width(context: Option<(&str, &str)>) -> u16 {
    let context_width = match context {
        Some((catalog, schema)) => 2 + catalog.chars().count() + 1 + schema.chars().count(),
        None => 0,
    };
    (" KAVEON".len() + context_width + 3) as u16
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
    fn status_line_lists_host_workers_and_last_query() {
        let facts = StatusFacts {
            host: "localhost:8081",
            workers_ready: Some((2, 0)),
            last_elapsed_ms: Some(1100),
            last_scanned_rows: Some(18_000_000),
        };
        let line = status_line(&facts, &Theme::mono());
        assert_eq!(
            crate::render::to_plain(&[line]),
            " localhost:8081 · 2 workers · last query 1.10 s, 18.0M rows scanned\n"
        );
        assert_eq!(host_of("http://localhost:8081/"), "localhost:8081");
        assert_eq!(
            crate::render::to_plain(&[prompt(Some(("OpenSource", "nyc")), &Theme::mono())]),
            " KAVEON: OpenSource.nyc › 
"
        );
        assert_eq!(prompt_width(Some(("OpenSource", "nyc"))), 26);
        assert_eq!(
            crate::render::to_plain(&[prompt(None, &Theme::mono())]),
            " KAVEON › 
"
        );
        assert_eq!(prompt_width(None), 10);
        assert_eq!(
            box_title("OpenSource", "kaveon_product"),
            "OpenSource.kaveon_product"
        );
    }
}
