//! The `.ask` client: a question in plain language goes to the platform
//! API's deterministic Data Language Model (`POST /api/v1/dlm/ask`), which
//! answers from its precomputed context or hands back the SQL it assembled.
//! The API is a separate service from the coordinator, with its own URL and
//! its own bearer token.
use crate::client::session::CliHttp;
use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::Duration;

const ASK_PATH: &str = "/api/v1/dlm/ask";

pub struct DlmClient {
    client: Client,
    api_url: String,
    token: Option<String>,
}

/// What the DLM said, in the five shapes the shell has to handle.
#[derive(Debug, Clone, PartialEq)]
pub enum AskAnswer {
    /// SQL assembled for the dataset; `engine` says it runs on the
    /// coordinator (a native Kaveon catalog) rather than a platform source.
    Live {
        dataset: String,
        catalog: String,
        schema: String,
        sql: String,
        engine: bool,
        title: Option<String>,
        note: Option<String>,
        confidence: Option<f64>,
        frame: Option<Value>,
        duration_ms: Option<f64>,
    },
    /// Rows served from the precomputed context, no database trip.
    Context {
        dataset: String,
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        title: Option<String>,
        note: Option<String>,
        approx: bool,
        confidence: Option<f64>,
        frame: Option<Value>,
        duration_ms: Option<f64>,
    },
    /// The question is ambiguous; `options` are `(id, label, description)`.
    Clarify {
        dataset: Option<String>,
        prompt: String,
        kind: String,
        options: Vec<(String, String, String)>,
        frame: Option<Value>,
    },
    OutOfScope {
        datasets: Vec<String>,
        hint: Option<String>,
    },
    /// `no_dataset`, `dataset_not_found`, `no_fact_table`, or a reason a
    /// newer API introduced.
    Refused { reason: String },
}

impl DlmClient {
    pub fn new(
        api_url: &str,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<DlmClient, CliHttp> {
        let loopback = validate_api_url(api_url)?;
        let mut builder = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none());
        if loopback {
            // Corporate proxy settings must not intercept a local API.
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|error| CliHttp::local(format!("cannot initialize HTTP client: {error}")))?;
        Ok(DlmClient {
            client,
            api_url: api_url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// `choices` pins a slot resolved after a clarification (`{"metric":
    /// "Net revenue"}`); `frame` is the previous answer's frame, which a
    /// follow-up inherits for every slot the new question does not name.
    pub fn ask(
        &self,
        question: &str,
        limit: usize,
        choices: Option<&Map<String, Value>>,
        frame: Option<&Value>,
    ) -> Result<AskAnswer, CliHttp> {
        let mut body = Map::new();
        body.insert("question".into(), Value::String(question.to_owned()));
        body.insert("limit".into(), Value::from(limit));
        if let Some(choices) = choices {
            body.insert("choices".into(), Value::Object(choices.clone()));
        }
        if let Some(frame) = frame {
            body.insert("frame".into(), frame.clone());
        }
        let mut request = self
            .client
            .post(format!("{}{ASK_PATH}", self.api_url))
            .json(&Value::Object(body));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().map_err(CliHttp::transport)?;
        let raw: Raw = decode(response)?;
        raw.into_answer()
    }
}

/// HTTPS, or HTTP to a loopback host for local development; the same rule
/// the coordinator URL follows. Returns whether the host is loopback.
fn validate_api_url(api_url: &str) -> Result<bool, CliHttp> {
    let url = reqwest::Url::parse(api_url).map_err(|_| CliHttp::local("invalid API URL"))?;
    let loopback = url
        .host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(CliHttp::local(
            "API URL requires HTTPS (HTTP is allowed only for local development)",
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CliHttp::local(
            "API URL must not contain credentials, query parameters, or fragments",
        ));
    }
    Ok(loopback)
}

/// The platform API reports failures FastAPI-style — `{"detail": "..."}` or
/// `{"detail": {"code", "message"}}` — unlike the coordinator's
/// `{"error", "code"}`, so the session decoder does not apply here.
fn decode<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, CliHttp> {
    let status = response.status();
    let body = response.text().map_err(CliHttp::transport)?;
    if !status.is_success() {
        let value = serde_json::from_str::<Value>(&body).ok();
        let detail = value.as_ref().and_then(|value| value.get("detail"));
        let message = detail
            .and_then(|detail| {
                detail
                    .as_str()
                    .or_else(|| detail.get("message").and_then(Value::as_str))
            })
            .or_else(|| {
                value
                    .as_ref()
                    .and_then(|value| value.get("error").and_then(Value::as_str))
            })
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if body.trim().is_empty() {
                    status.to_string()
                } else {
                    body.clone()
                }
            });
        let code = detail
            .and_then(|detail| detail.get("code").and_then(Value::as_str))
            .or_else(|| {
                value
                    .as_ref()
                    .and_then(|value| value.get("code").and_then(Value::as_str))
            })
            .map(str::to_owned);
        return Err(CliHttp {
            status: Some(status.as_u16()),
            code,
            message,
            timed_out: false,
            connect: false,
        });
    }
    serde_json::from_str(&body).map_err(|error| CliHttp {
        status: Some(status.as_u16()),
        code: None,
        message: format!("invalid platform API response: {error}"),
        timed_out: false,
        connect: false,
    })
}

/// Every key the answer may carry, all optional: the API adds chart hints
/// and diagnostics the shell does not need, and a newer API may add more.
#[derive(Debug, Default, Deserialize)]
struct Raw {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    from_context: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    dataset_id: Option<Value>,
    #[serde(default)]
    dataset_name: Option<String>,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    schema_name: Option<String>,
    #[serde(default)]
    sql: Option<String>,
    #[serde(default)]
    engine: bool,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    rows: Vec<Vec<Value>>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    approx: bool,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    frame: Option<Value>,
    #[serde(default)]
    duration_ms: Option<f64>,
    #[serde(default)]
    clarification: Option<Clarification>,
    #[serde(default)]
    datasets: Vec<String>,
    #[serde(default)]
    hint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Clarification {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    options: Vec<ClarificationOption>,
}

#[derive(Debug, Default, Deserialize)]
struct ClarificationOption {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
}

impl Raw {
    /// The dataset as people know it, falling back to its id.
    fn dataset(&self) -> Option<String> {
        self.dataset_name
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.dataset_id.as_ref().map(|id| match id {
                    Value::String(id) => id.clone(),
                    other => other.to_string(),
                })
            })
    }

    fn into_answer(self) -> Result<AskAnswer, CliHttp> {
        if !self.ok {
            if let Some(clarification) = self.clarification {
                return Ok(AskAnswer::Clarify {
                    dataset: self.dataset_name.filter(|name| !name.is_empty()),
                    prompt: clarification.prompt,
                    kind: clarification.kind,
                    options: clarification
                        .options
                        .into_iter()
                        .map(|option| (option.id, option.label, option.description))
                        .collect(),
                    frame: self.frame,
                });
            }
            let reason = self.reason.unwrap_or_default();
            if reason == "out_of_scope" {
                return Ok(AskAnswer::OutOfScope {
                    datasets: self.datasets,
                    hint: self.hint,
                });
            }
            return Ok(AskAnswer::Refused {
                reason: if reason.is_empty() {
                    "unknown".to_owned()
                } else {
                    reason
                },
            });
        }
        let dataset = self.dataset().unwrap_or_default();
        if self.from_context || self.route.as_deref() == Some("context") {
            return Ok(AskAnswer::Context {
                dataset,
                columns: self.columns,
                rows: self.rows,
                title: self.title,
                note: self.note,
                approx: self.approx,
                confidence: self.confidence,
                frame: self.frame,
                duration_ms: self.duration_ms,
            });
        }
        let Some(sql) = self.sql else {
            return Err(CliHttp::local(
                "invalid platform API response: the answer carries neither rows nor SQL",
            ));
        };
        Ok(AskAnswer::Live {
            dataset,
            catalog: self.database.unwrap_or_default(),
            schema: self.schema_name.unwrap_or_default(),
            sql,
            engine: self.engine,
            title: self.title,
            note: self.note,
            confidence: self.confidence,
            frame: self.frame,
            duration_ms: self.duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves one response and hands back the whole request (request line,
    /// headers, body) so a test can look at the headers too.
    fn serve(status: u16, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_owned();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let n = stream.read(&mut chunk).unwrap();
                bytes.extend_from_slice(&chunk[..n]);
                if n == 0 {
                    break;
                }
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .map_or(0, |value| value.trim().parse::<usize>().unwrap());
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        });
        (url, thread)
    }

    fn request_line(request: &str) -> &str {
        request.lines().next().unwrap_or_default()
    }

    fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request
            .split("\r\n\r\n")
            .next()
            .unwrap_or_default()
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then(|| value.trim())
            })
    }

    fn body(request: &str) -> Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or_default()).unwrap()
    }

    #[test]
    fn live_answer_posts_the_question_with_the_bearer_token() {
        let response = json!({
            "ok": true, "dataset_id": "d1", "dataset_name": "Sales", "database": "kaveon",
            "schema_name": "sales", "sql": "SELECT region, SUM(net) AS net FROM orders GROUP BY 1",
            "engine": true, "from_context": false, "route": "live", "chartType": "bar",
            "title": "Net revenue by region", "columns": ["region", "net"], "note": null,
            "context_hints": {}, "confidence": 0.83, "frame": {"dataset_id": "d1", "metric": "net"},
            "duration_ms": 12.5, "_rebuild_triggered": true
        });
        let (url, thread) = serve(200, &response.to_string());
        let client = DlmClient::new(&url, Some("tok-123".into()), Duration::from_secs(5)).unwrap();
        let answer = client.ask("net revenue by region", 50, None, None).unwrap();
        let request = thread.join().unwrap();
        assert!(request_line(&request).starts_with("POST /api/v1/dlm/ask "));
        assert_eq!(header(&request, "authorization"), Some("Bearer tok-123"));
        assert_eq!(
            body(&request),
            json!({"question": "net revenue by region", "limit": 50})
        );
        assert_eq!(
            answer,
            AskAnswer::Live {
                dataset: "Sales".into(),
                catalog: "kaveon".into(),
                schema: "sales".into(),
                sql: "SELECT region, SUM(net) AS net FROM orders GROUP BY 1".into(),
                engine: true,
                title: Some("Net revenue by region".into()),
                note: None,
                confidence: Some(0.83),
                frame: Some(json!({"dataset_id": "d1", "metric": "net"})),
                duration_ms: Some(12.5),
            }
        );
    }

    #[test]
    fn context_answer_carries_rows_and_sends_choices_and_frame_without_a_token() {
        let response = json!({
            "ok": true, "dataset_id": "d1", "dataset_name": "Sales", "from_context": true,
            "route": "context", "columns": ["region", "net"],
            "rows": [["East", 120.5], ["West", null]], "chartType": "bar",
            "title": "net by region", "note": "≈ estimated", "confidence": 0.91, "approx": true,
            "frame": {"dataset_id": "d1"}, "duration_ms": 0.4
        });
        let (url, thread) = serve(200, &response.to_string());
        let client = DlmClient::new(&url, None, Duration::from_secs(5)).unwrap();
        let mut choices = Map::new();
        choices.insert("metric".into(), Value::String("Net revenue".into()));
        let frame = json!({"dataset_id": "d1", "metric": "net"});
        let answer = client
            .ask("by region", 10, Some(&choices), Some(&frame))
            .unwrap();
        let request = thread.join().unwrap();
        assert_eq!(header(&request, "authorization"), None);
        assert_eq!(
            body(&request),
            json!({
                "question": "by region", "limit": 10,
                "choices": {"metric": "Net revenue"},
                "frame": {"dataset_id": "d1", "metric": "net"}
            })
        );
        assert_eq!(
            answer,
            AskAnswer::Context {
                dataset: "Sales".into(),
                columns: vec!["region".into(), "net".into()],
                rows: vec![
                    vec![json!("East"), json!(120.5)],
                    vec![json!("West"), Value::Null]
                ],
                title: Some("net by region".into()),
                note: Some("≈ estimated".into()),
                approx: true,
                confidence: Some(0.91),
                frame: Some(json!({"dataset_id": "d1"})),
                duration_ms: Some(0.4),
            }
        );
    }

    #[test]
    fn clarification_lists_the_options() {
        let response = json!({
            "ok": false, "reason": "clarify", "dataset_id": "d1", "dataset_name": "Sales",
            "clarification": {
                "kind": "metric", "prompt": "Which revenue?",
                "options": [
                    {"id": "net", "label": "Net revenue", "description": "after returns"},
                    {"id": "gross", "label": "Gross revenue", "description": "before returns"}
                ]
            },
            "resume": {"question": "revenue", "choices": {}}, "duration_ms": 3.0
        });
        let (url, thread) = serve(200, &response.to_string());
        let client = DlmClient::new(&url, None, Duration::from_secs(5)).unwrap();
        let answer = client.ask("revenue", 50, None, None).unwrap();
        thread.join().unwrap();
        assert_eq!(
            answer,
            AskAnswer::Clarify {
                dataset: Some("Sales".into()),
                prompt: "Which revenue?".into(),
                kind: "metric".into(),
                options: vec![
                    ("net".into(), "Net revenue".into(), "after returns".into()),
                    (
                        "gross".into(),
                        "Gross revenue".into(),
                        "before returns".into()
                    ),
                ],
                frame: None,
            }
        );
    }

    #[test]
    fn out_of_scope_names_the_datasets_and_the_hint() {
        let response = json!({
            "ok": false, "reason": "out_of_scope", "datasets": ["Sales", "Energy"],
            "hint": "That is SQL. Run it in SQL Lab; Chat answers questions in plain language.",
            "duration_ms": 0.2
        });
        let (url, thread) = serve(200, &response.to_string());
        let client = DlmClient::new(&url, None, Duration::from_secs(5)).unwrap();
        let answer = client.ask("select 1", 50, None, None).unwrap();
        thread.join().unwrap();
        assert_eq!(
            answer,
            AskAnswer::OutOfScope {
                datasets: vec!["Sales".into(), "Energy".into()],
                hint: Some(
                    "That is SQL. Run it in SQL Lab; Chat answers questions in plain language."
                        .into()
                ),
            }
        );
    }

    #[test]
    fn other_refusals_keep_the_reason() {
        for reason in ["no_dataset", "dataset_not_found", "no_fact_table"] {
            let (url, thread) = serve(200, &json!({"ok": false, "reason": reason}).to_string());
            let client = DlmClient::new(&url, None, Duration::from_secs(5)).unwrap();
            let answer = client.ask("anything", 50, None, None).unwrap();
            thread.join().unwrap();
            assert_eq!(
                answer,
                AskAnswer::Refused {
                    reason: reason.into()
                }
            );
        }
    }

    #[test]
    fn unauthorized_carries_the_status_and_the_api_message() {
        let (url, thread) = serve(
            401,
            r#"{"detail":{"code":"unauthorized","message":"Authentication required."}}"#,
        );
        let client = DlmClient::new(&url, None, Duration::from_secs(5)).unwrap();
        let failure = client.ask("anything", 50, None, None).unwrap_err();
        thread.join().unwrap();
        assert_eq!(failure.status, Some(401));
        assert_eq!(failure.code.as_deref(), Some("unauthorized"));
        assert_eq!(failure.message, "Authentication required.");
    }

    #[test]
    fn plain_http_is_only_for_loopback() {
        assert!(DlmClient::new("http://api.example.com", None, Duration::from_secs(1)).is_err());
        assert!(DlmClient::new("https://api.example.com/", None, Duration::from_secs(1)).is_ok());
        assert!(DlmClient::new("http://localhost:8080", None, Duration::from_secs(1)).is_ok());
        assert!(
            DlmClient::new("https://api.example.com/?x=1", None, Duration::from_secs(1)).is_err()
        );
    }
}
