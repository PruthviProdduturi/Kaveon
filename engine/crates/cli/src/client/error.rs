//! One shape for every failure the client reports: a kind, the message with
//! transport noise removed, the workers that failed, and where in the SQL
//! the server pointed when it did.
use crate::client::session::CliHttp;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ErrorKind {
    Parse,
    Planning,
    Worker,
    /// The embedded engine failed while running a statement.
    Execution,
    Admission,
    Cancelled,
    Connection,
    Authentication,
    NotFound,
    /// The object exists, is not empty, or changed under the statement.
    Conflict,
    Coordinator,
}

impl ErrorKind {
    pub fn label(&self) -> &'static str {
        match self {
            ErrorKind::Parse => "SQL parse error",
            ErrorKind::Planning => "Planning error",
            ErrorKind::Worker => "Worker failure",
            ErrorKind::Execution => "Execution error",
            ErrorKind::Admission => "Memory admission",
            ErrorKind::Cancelled => "Cancelled",
            ErrorKind::Connection => "Connection",
            ErrorKind::Authentication => "Authentication",
            ErrorKind::NotFound => "Not found",
            ErrorKind::Conflict => "Conflict",
            ErrorKind::Coordinator => "Coordinator",
        }
    }

    fn from_code(code: &str) -> Option<ErrorKind> {
        Some(match code {
            "SQL_PARSE_ERROR" | "SYNTAX_ERROR" => ErrorKind::Parse,
            "PLANNING_ERROR" | "ANALYSIS_ERROR" => ErrorKind::Planning,
            "MEMORY_ADMISSION_REJECTED" => ErrorKind::Admission,
            "QUERY_CANCELED" => ErrorKind::Cancelled,
            "QUERY_NOT_FOUND" | "CATALOG_NOT_FOUND" | "SCHEMA_NOT_FOUND" | "TABLE_NOT_FOUND"
            | "TABLE_NOT_READABLE" => ErrorKind::NotFound,
            "CATALOG_CONFLICT" => ErrorKind::Conflict,
            "CATALOG_INVALID" => ErrorKind::Planning,
            "FORBIDDEN" => ErrorKind::Authentication,
            _ => return None,
        })
    }

    fn from_status(status: u16) -> Option<ErrorKind> {
        Some(match status {
            401 | 403 => ErrorKind::Authentication,
            429 => ErrorKind::Admission,
            409 => ErrorKind::Conflict,
            404 => ErrorKind::NotFound,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub query_id: Option<String>,
    pub workers: Vec<String>,
    /// 1-based (line, column) into `sql`, when the server supplied one.
    pub position: Option<(usize, usize)>,
    pub sql: Option<String>,
}

/// Prefixes the server and the legacy client wrap around the real message,
/// each with the kind it implies when nothing better is known.
const PREFIXES: [(&str, Option<ErrorKind>); 9] = [
    ("SQL parse error: ", Some(ErrorKind::Parse)),
    ("sql parser error: ", Some(ErrorKind::Parse)),
    ("SQL analysis error: ", Some(ErrorKind::Planning)),
    ("planning error: ", Some(ErrorKind::Planning)),
    ("execution error: ", None),
    ("execution: ", None),
    ("sql: ", None),
    (
        "cannot connect to coordinator: ",
        Some(ErrorKind::Connection),
    ),
    ("coordinator request failed: ", Some(ErrorKind::Connection)),
];

/// Legacy transport messages that carry no detail after a colon.
const CONNECTION_MESSAGES: [&str; 2] = [
    "coordinator request timed out",
    "coordinator request failed",
];

impl CliError {
    pub fn from_http(failure: CliHttp, sql: Option<&str>) -> CliError {
        let CliHttp {
            status,
            code,
            message,
            timed_out,
            connect,
        } = failure;
        Self::build(&message, status, code.as_deref(), timed_out || connect, sql)
    }

    /// For the `String` errors the client still produces on its older paths.
    pub fn from_message(message: &str, sql: Option<&str>) -> CliError {
        Self::build(message, None, None, false, sql)
    }

    pub fn with_query_id(mut self, id: impl Into<String>) -> CliError {
        self.query_id = Some(id.into());
        self
    }

    fn build(
        raw: &str,
        status: Option<u16>,
        code: Option<&str>,
        transport: bool,
        sql: Option<&str>,
    ) -> CliError {
        let mut message = raw.trim().to_owned();
        let mut query_id = None;
        let mut status = status;

        if let Some((id, rest)) = query_failed_prefix(&message) {
            query_id = Some(id);
            message = rest;
        }
        let mut code = code.map(str::to_owned);
        if let Some((http_status, bracketed, rest)) = coordinator_prefix(&message) {
            status = status.or(Some(http_status));
            if code.is_none() {
                code = bracketed;
            }
            message = rest;
        }

        let mut kind = code.as_deref().and_then(ErrorKind::from_code);
        let mut workers = Vec::new();
        if let Some((names, messages)) = worker_failures(&message) {
            workers = names;
            message = messages
                .into_iter()
                .map(|piece| strip_prefixes(&piece).0)
                .collect::<Vec<_>>()
                .join("; ");
            kind = kind.or(Some(ErrorKind::Worker));
        }

        let (stripped, hinted) = strip_prefixes(&message);
        message = stripped;
        let kind = kind
            .or_else(|| status.and_then(ErrorKind::from_status))
            .or(if transport {
                Some(ErrorKind::Connection)
            } else {
                None
            })
            .or(hinted)
            .or(
                if CONNECTION_MESSAGES
                    .iter()
                    .any(|prefix| message.starts_with(prefix))
                {
                    Some(ErrorKind::Connection)
                } else {
                    None
                },
            )
            .unwrap_or(ErrorKind::Coordinator);

        let (message, position) = split_position(&message);
        CliError {
            kind,
            message,
            query_id,
            workers,
            position,
            sql: sql.map(str::to_owned),
        }
    }
}

impl From<CliHttp> for CliError {
    fn from(failure: CliHttp) -> CliError {
        CliError::from_http(failure, None)
    }
}

/// `query <id> failed: <message>` → (id, message).
fn query_failed_prefix(message: &str) -> Option<(String, String)> {
    let rest = message.strip_prefix("query ")?;
    let (id, rest) = rest.split_once(" failed: ")?;
    if id.is_empty() || id.contains(char::is_whitespace) {
        return None;
    }
    Some((id.to_owned(), rest.trim().to_owned()))
}

/// `coordinator returned HTTP <n> <text> [<CODE>]: <detail>` → (n, code, detail).
fn coordinator_prefix(message: &str) -> Option<(u16, Option<String>, String)> {
    let rest = message.strip_prefix("coordinator returned HTTP ")?;
    let (status_text, detail) = rest.split_once(": ")?;
    let status = status_text
        .split_whitespace()
        .next()
        .and_then(|digits| digits.parse::<u16>().ok())?;
    let code = status_text
        .rsplit_once('[')
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .map(str::to_owned);
    Some((status, code, detail.trim().to_owned()))
}

/// `worker '<name>' failed task with <status>: <body>` pieces joined by
/// `; ` → (worker names, distinct messages in first-seen order).
fn worker_failures(message: &str) -> Option<(Vec<String>, Vec<String>)> {
    const HEAD: &str = "worker '";
    const SEPARATOR: &str = "; worker '";
    if !message.starts_with(HEAD) {
        return None;
    }
    let mut names = Vec::new();
    let mut messages: Vec<String> = Vec::new();
    for (index, piece) in message.split(SEPARATOR).enumerate() {
        let piece = if index == 0 {
            piece.strip_prefix(HEAD)?
        } else {
            piece
        };
        let (name, rest) = piece.split_once("' failed task with ")?;
        let (status_text, body) = rest.split_once(':').unwrap_or((rest, ""));
        let body = body.trim();
        let text = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| {
                if body.is_empty() {
                    status_text.trim().to_owned()
                } else {
                    body.to_owned()
                }
            });
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
        if !messages.contains(&text) {
            messages.push(text);
        }
    }
    Some((names, messages))
}

/// Removes every known wrapper prefix, outermost first, and reports the
/// kind the outermost one implied.
fn strip_prefixes(message: &str) -> (String, Option<ErrorKind>) {
    let mut rest = message.trim();
    let mut hint = None;
    while let Some((prefix, kind)) = PREFIXES.iter().find(|(prefix, _)| rest.starts_with(prefix)) {
        hint = hint.or(*kind);
        rest = rest[prefix.len()..].trim_start();
    }
    (rest.to_owned(), hint)
}

/// A trailing ` at Line: N, Column: M` (sqlparser) or ` at line N, column M`
/// → (message without it, (N, M)).
fn split_position(message: &str) -> (String, Option<(usize, usize)>) {
    let mut search = message.len();
    while let Some(index) = message[..search].rfind(" at ") {
        if let Some(position) = parse_position(&message[index + 4..]) {
            return (message[..index].trim_end().to_owned(), Some(position));
        }
        search = index;
    }
    (message.to_owned(), None)
}

fn parse_position(tail: &str) -> Option<(usize, usize)> {
    let rest = strip_word(tail, "line")?;
    let (line, rest) = take_number(rest)?;
    let rest = rest.trim_start().strip_prefix(',')?;
    let rest = strip_word(rest.trim_start(), "column")?;
    let (column, rest) = take_number(rest)?;
    rest.trim().is_empty().then_some((line, column))
}

fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let rest = text.get(word.len()..)?;
    if !text[..word.len()].eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    Some(rest.trim_start())
}

fn take_number(text: &str) -> Option<(usize, &str)> {
    let end = text
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map_or(text.len(), |(index, _)| index);
    if end == 0 {
        return None;
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render;
    use crate::theme::Theme;

    fn http(status: Option<u16>, code: Option<&str>, message: &str) -> CliHttp {
        CliHttp {
            status,
            code: code.map(str::to_owned),
            message: message.to_owned(),
            timed_out: false,
            connect: false,
        }
    }

    #[test]
    fn worker_failures_are_deduplicated_and_unwrapped() {
        let failure = http(
            Some(500),
            None,
            "worker 'worker-1' failed task with 500 Internal Server Error: {\"error\":\"storage: projection references unknown column 'nope'\"}; worker 'worker-2' failed task with 500 Internal Server Error: {\"error\":\"storage: projection references unknown column 'nope'\"}",
        );
        let error = CliError::from_http(failure, Some("SELECT nope FROM t"));
        assert_eq!(error.kind, ErrorKind::Worker);
        assert_eq!(
            error.message,
            "storage: projection references unknown column 'nope'"
        );
        assert_eq!(error.workers, ["worker-1", "worker-2"]);
        assert_eq!(error.sql.as_deref(), Some("SELECT nope FROM t"));
        assert_eq!(
            render::error::plain(&error),
            "error: Worker failure: storage: projection references unknown column 'nope'"
        );
    }

    #[test]
    fn distinct_worker_messages_are_kept_and_execution_prefixes_dropped() {
        let error = CliError::from_message(
            "coordinator returned HTTP 500 Internal Server Error: worker 'worker-1' failed task with 500 Internal Server Error: {\"error\":\"execution: spill failed; disk full\"}; worker 'worker-2' failed task with 503 Service Unavailable: ",
            None,
        );
        assert_eq!(error.kind, ErrorKind::Worker);
        assert_eq!(error.workers, ["worker-1", "worker-2"]);
        assert_eq!(
            error.message,
            "spill failed; disk full; 503 Service Unavailable"
        );
    }

    #[test]
    fn parse_errors_drop_the_transport_prefix_and_keep_a_position() {
        let failure = http(
            Some(400),
            Some("SQL_PARSE_ERROR"),
            "SQL parse error: sql: Expected an expression, found: FROM at line 1, column 8",
        );
        let error = CliError::from_http(failure, Some("SELECT FROM t"));
        assert_eq!(error.kind, ErrorKind::Parse);
        assert_eq!(error.message, "Expected an expression, found: FROM");
        assert_eq!(error.position, Some((1, 8)));
        let panel = render::to_plain(&render::error::panel(&error, &Theme::mono()));
        assert!(panel.contains("✗ SQL parse error"), "{panel}");
        assert!(panel.contains("SELECT FROM t\n          ^"), "{panel}");
    }

    #[test]
    fn sqlparser_positions_and_the_server_syntax_code_are_understood() {
        let failure = http(
            Some(400),
            Some("SYNTAX_ERROR"),
            "SQL parse error: sql: sql parser error: Expected: an expression, found: FROM at Line: 1, Column: 8",
        );
        let error = CliError::from_http(failure, None);
        assert_eq!(error.kind, ErrorKind::Parse);
        assert_eq!(error.message, "Expected: an expression, found: FROM");
        assert_eq!(error.position, Some((1, 8)));
        let legacy = CliError::from_message(
            "coordinator returned HTTP 400 Bad Request: SQL parse error: sql: sql parser error: Expected: an expression, found: FROM at Line: 2, Column: 3",
            None,
        );
        assert_eq!(legacy.kind, ErrorKind::Parse);
        assert_eq!(legacy.position, Some((2, 3)));
        let none = CliError::from_message(
            "planning error: table 'events' is at line 3, column x",
            None,
        );
        assert_eq!(none.kind, ErrorKind::Planning);
        assert_eq!(none.position, None);
        assert_eq!(none.message, "table 'events' is at line 3, column x");
    }

    #[test]
    fn status_codes_map_to_kinds() {
        for (status, code, kind) in [
            (429, Some("MEMORY_ADMISSION_REJECTED"), ErrorKind::Admission),
            (409, Some("QUERY_CANCELED"), ErrorKind::Cancelled),
            (409, Some("CATALOG_CONFLICT"), ErrorKind::Conflict),
            (409, None, ErrorKind::Conflict),
            (400, Some("TABLE_NOT_READABLE"), ErrorKind::NotFound),
            (401, None, ErrorKind::Authentication),
            (403, None, ErrorKind::Authentication),
            (404, None, ErrorKind::NotFound),
            (400, Some("PLANNING_ERROR"), ErrorKind::Planning),
            (400, Some("CATALOG_NOT_FOUND"), ErrorKind::NotFound),
            (503, None, ErrorKind::Coordinator),
        ] {
            let failure = http(Some(status), code, "m");
            assert_eq!(CliError::from_http(failure, None).kind, kind, "{status}");
        }
        let connect = CliHttp {
            status: None,
            code: None,
            message: "refused".into(),
            timed_out: false,
            connect: true,
        };
        assert_eq!(
            CliError::from_http(connect, None).kind,
            ErrorKind::Connection
        );
        let timeout = CliHttp {
            timed_out: true,
            connect: false,
            ..CliHttp::local("slow")
        };
        assert_eq!(CliError::from(timeout).kind, ErrorKind::Connection);
    }

    #[test]
    fn legacy_messages_carry_status_query_id_and_connection_kinds() {
        let error = CliError::from_message(
            "coordinator returned HTTP 401 Unauthorized: missing bearer token",
            None,
        );
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert_eq!(error.message, "missing bearer token");

        let error = CliError::from_message(
            "query 7c0c9b9e-1 failed: execution: out of memory",
            Some("SELECT 1"),
        );
        assert_eq!(error.kind, ErrorKind::Coordinator);
        assert_eq!(error.query_id.as_deref(), Some("7c0c9b9e-1"));
        assert_eq!(error.message, "out of memory");

        for message in [
            "coordinator request timed out",
            "cannot connect to coordinator: connection refused",
            "coordinator request failed: reset by peer",
        ] {
            assert_eq!(
                CliError::from_message(message, None).kind,
                ErrorKind::Connection,
                "{message}"
            );
        }
        assert_eq!(
            CliError::from_message("cannot connect to coordinator: connection refused", None)
                .message,
            "connection refused"
        );
        let plain = CliError::from_message("usage: USE [catalog.]schema", None);
        assert_eq!(plain.kind, ErrorKind::Coordinator);
        assert_eq!(plain.message, "usage: USE [catalog.]schema");
    }
}
