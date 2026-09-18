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
pub fn prompt(context: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {context} "), theme.accent),
        Span::styled("› ", theme.accent),
    ])
}

pub fn prompt_width(context: &str) -> u16 {
    (context.chars().count() + 4) as u16
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
            crate::render::to_plain(&[prompt("kaveon.default", &Theme::mono())]),
            " kaveon.default › \n"
        );
        assert_eq!(prompt_width("kaveon.default"), 18);
        assert_eq!(
            box_title("OpenSource", "kaveon_product"),
            "OpenSource.kaveon_product"
        );
    }
}
