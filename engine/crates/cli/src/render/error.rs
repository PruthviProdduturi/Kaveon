//! The error panel: kind and query id, the message with the workers that
//! reported it, then the SQL excerpt with a caret when the server pointed
//! at a position. `plain` is the single stderr line for non-TTY output.
use crate::client::error::CliError;
use crate::theme::Theme;
use ratatui::text::{Line, Span};

/// How many statement lines the panel shows.
const SQL_LINES: usize = 3;
const INDENT: &str = "   ";

pub fn panel(error: &CliError, theme: &Theme) -> Vec<Line<'static>> {
    let mut first = vec![Span::styled(
        format!(" ✗ {}", error.kind.label()),
        theme.error,
    )];
    if let Some(id) = &error.query_id {
        first.push(Span::styled(
            format!("{INDENT}query {}", id.chars().take(8).collect::<String>()),
            theme.dim,
        ));
    }
    let mut second = vec![Span::raw(format!("{INDENT}{}", error.message))];
    if !error.workers.is_empty() {
        second.push(Span::styled(
            format!(" ({})", error.workers.join(", ")),
            theme.dim,
        ));
    }
    let mut lines = vec![Line::from(first), Line::from(second)];
    if let Some(sql) = &error.sql {
        lines.extend(excerpt(sql, error.position, theme));
    }
    lines
}

/// Up to three statement lines, dimmed; when a position is known the
/// window ends on its line and a caret sits under its column.
fn excerpt(sql: &str, position: Option<(usize, usize)>, theme: &Theme) -> Vec<Line<'static>> {
    let all: Vec<&str> = sql.lines().collect();
    if all.is_empty() {
        return Vec::new();
    }
    let target = position
        .map(|(line, _)| line.clamp(1, all.len()))
        .unwrap_or(1);
    let last = target.max(SQL_LINES).min(all.len());
    let first = last.saturating_sub(SQL_LINES);
    let mut lines = Vec::new();
    for (index, text) in all[first..last].iter().enumerate() {
        lines.push(Line::from(Span::styled(
            format!("{INDENT}{}", escape(text)),
            theme.dim,
        )));
        if let Some((_, column)) = position
            && first + index + 1 == target
        {
            lines.push(Line::from(Span::styled(
                format!(
                    "{INDENT}{}^",
                    " ".repeat(column.saturating_sub(1).min(text.chars().count()))
                ),
                theme.error,
            )));
        }
    }
    lines
}

/// Tabs become a space so the caret column stays true; other control
/// characters are dropped rather than sent to the terminal.
fn escape(text: &str) -> String {
    text.chars()
        .map(|ch| if ch == '\t' { ' ' } else { ch })
        .filter(|ch| !ch.is_control())
        .collect()
}

/// `error: <kind>: <message>`, plus ` (query <id>)` when known.
pub fn plain(error: &CliError) -> String {
    let mut out = format!("error: {}: {}", error.kind.label(), error.message);
    if let Some(id) = &error.query_id {
        out.push_str(&format!(" (query {id})"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::error::ErrorKind;
    use crate::render::to_plain;

    fn error(kind: ErrorKind, message: &str) -> CliError {
        CliError {
            kind,
            message: message.to_owned(),
            query_id: None,
            workers: Vec::new(),
            position: None,
            sql: None,
        }
    }

    #[test]
    fn panel_reads_kind_query_message_workers_and_excerpt() {
        let mut worker = error(
            ErrorKind::Worker,
            "storage: projection references unknown column 'nope'",
        )
        .with_query_id("7c0c9b9e-4a1c");
        worker.workers = vec!["worker-1".into(), "worker-2".into()];
        worker.sql = Some("SELECT nope\nFROM kaveon_events_users\nWHERE 1 = 1\nLIMIT 5".into());
        assert_eq!(
            to_plain(&panel(&worker, &Theme::mono())),
            " ✗ Worker failure   query 7c0c9b9e\n   storage: projection references unknown column 'nope' (worker-1, worker-2)\n   SELECT nope\n   FROM kaveon_events_users\n   WHERE 1 = 1\n"
        );
        assert_eq!(
            plain(&worker),
            "error: Worker failure: storage: projection references unknown column 'nope' (query 7c0c9b9e-4a1c)"
        );
    }

    #[test]
    fn caret_sits_under_the_column_and_the_window_ends_on_that_line() {
        let mut parse = error(ErrorKind::Parse, "Expected an expression, found: FROM");
        parse.sql = Some("SELECT FROM t".into());
        parse.position = Some((1, 8));
        assert_eq!(
            to_plain(&panel(&parse, &Theme::mono())),
            " ✗ SQL parse error\n   Expected an expression, found: FROM\n   SELECT FROM t\n          ^\n"
        );
        parse.sql = Some("SELECT\n  a,\n  b,\n  c\nFROM\n  t".into());
        parse.position = Some((5, 1));
        assert_eq!(
            to_plain(&panel(&parse, &Theme::mono())),
            " ✗ SQL parse error\n   Expected an expression, found: FROM\n     b,\n     c\n   FROM\n   ^\n"
        );
    }

    #[test]
    fn plain_is_one_line_without_a_query_id() {
        let cancelled = error(ErrorKind::Cancelled, "query canceled");
        assert_eq!(plain(&cancelled), "error: Cancelled: query canceled");
        assert_eq!(
            to_plain(&panel(&cancelled, &Theme::mono())),
            " ✗ Cancelled\n   query canceled\n"
        );
    }

    #[test]
    fn styled_panel_colours_the_kind_and_dims_the_provenance() {
        let theme = Theme::detect("dark", true);
        if theme.plain {
            return;
        }
        let mut worker = error(ErrorKind::Worker, "m").with_query_id("abcdefgh12");
        worker.workers = vec!["w".into()];
        let lines = panel(&worker, &theme);
        assert_eq!(lines[0].spans[0].style, theme.error);
        assert_eq!(lines[0].spans[1].style, theme.dim);
        assert_eq!(lines[1].spans[1].style, theme.dim);
    }
}
