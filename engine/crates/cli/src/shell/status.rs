//! The editor box title and the one-line status bar under it.
use crate::theme::Theme;
use ratatui::text::{Line, Span};

pub struct StatusFacts<'a> {
    pub context: &'a str,
    pub host: &'a str,
    pub workers_ready: Option<(usize, usize)>,
    pub last_elapsed_ms: Option<u64>,
    pub last_scanned_rows: Option<u64>,
}

pub fn box_title(catalog: &str, schema: &str) -> String {
    format!("{catalog}.{schema}")
}

pub fn status_line(facts: &StatusFacts<'_>, theme: &Theme) -> Line<'static> {
    let mut parts = vec![facts.context.to_owned(), facts.host.to_owned()];
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
    let mut spans = vec![
        Span::styled(format!(" {}", parts[0]), theme.accent),
        Span::styled(format!(" · {}", parts[1..].join(" · ")), style),
    ];
    if parts.len() == 1 {
        spans.pop();
    }
    Line::from(spans)
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
            context: "OpenSource.kaveon_product",
            host: "localhost:8081",
            workers_ready: Some((2, 0)),
            last_elapsed_ms: Some(1100),
            last_scanned_rows: Some(18_000_000),
        };
        let line = status_line(&facts, &Theme::mono());
        assert_eq!(
            crate::render::to_plain(&[line]),
            " OpenSource.kaveon_product · localhost:8081 · 2 workers · last query 1.10 s, 18.0M rows scanned\n"
        );
        assert_eq!(host_of("http://localhost:8081/"), "localhost:8081");
        assert_eq!(
            box_title("OpenSource", "kaveon_product"),
            "OpenSource.kaveon_product"
        );
    }
}
