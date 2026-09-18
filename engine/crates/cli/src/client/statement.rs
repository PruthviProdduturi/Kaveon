//! A statement on a worker thread. The UI thread keeps drawing while the
//! blocking `POST /v1/statement` runs; the request carries a unique client
//! tag so the coordinator's history can be searched for the record before
//! the response arrives. The shell asks for paged delivery: the response
//! carries a `next_uri` and the rows come through `client::pages`.
use crate::args::Options;
use crate::auth::Session;
use crate::client::session::CliHttp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// `auth::Session` refreshes its token behind a `RefCell`, so the two
/// threads share it through a mutex. The worker holds the lock only while
/// it builds the request; the UI thread's polls take it briefly.
pub type SharedSession = Arc<Mutex<Session>>;

#[derive(Serialize, Clone, Debug)]
pub struct StatementRequest {
    pub query: String,
    pub catalog: String,
    pub schema: String,
    pub user: String,
    pub source: String,
    pub client: &'static str,
    pub client_tags: Vec<String>,
    pub result_delivery: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Map<String, Value>>,
}

impl StatementRequest {
    pub fn new(sql: &str, options: &Options) -> StatementRequest {
        StatementRequest {
            query: sql.to_owned(),
            catalog: options.catalog.clone(),
            schema: options.schema.clone(),
            user: options.user.clone(),
            source: options.source.clone(),
            client: "kaveon-cli",
            client_tags: options.client_tags.clone(),
            result_delivery: "paged",
            settings: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
}

#[derive(Debug, Deserialize)]
pub struct StatementResult {
    pub id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub data: Vec<Vec<Value>>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub next_uri: Option<String>,
}

impl StatementResult {
    pub fn column_names(&self) -> Vec<String> {
        self.columns
            .iter()
            .map(|column| column.name.clone())
            .collect()
    }
}

pub enum StatementEvent {
    Finished(StatementResult),
    /// A transport or HTTP failure, or a statement the coordinator ran and
    /// reported as failed (a 200 with `error`; the message then reads
    /// `query <id> failed: <error>` and `status` is 200).
    Failed(CliHttp),
}

pub struct Handle {
    /// The `kaveon-cli:<uuid>` tag this statement carries in `client_tags`.
    pub tag: String,
    pub events: mpsc::Receiver<StatementEvent>,
    pub started: Instant,
}

pub fn new_tag() -> String {
    format!("kaveon-cli:{}", uuid::Uuid::new_v4())
}

/// Spawns the worker thread that posts `request` and reports once through
/// the handle's channel. The tag is appended to the request's `client_tags`
/// before it is sent.
pub fn submit(
    session: SharedSession,
    server: String,
    mut request: StatementRequest,
    timeout: Duration,
) -> Handle {
    let tag = new_tag();
    request.client_tags.push(tag.clone());
    let (sender, events) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = sender.send(post(&session, &server, &request, timeout));
    });
    Handle {
        tag,
        events,
        started,
    }
}

fn post(
    session: &SharedSession,
    server: &str,
    request: &StatementRequest,
    timeout: Duration,
) -> StatementEvent {
    let url = crate::client::session::endpoint(server, "/v1/statement");
    // Build under the lock (the token may need refreshing), send outside it
    // so the UI thread can poll the coordinator while the statement runs.
    let builder = {
        let guard = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.request(reqwest::Method::POST, &url)
    };
    let builder = match builder {
        Ok(builder) => builder,
        Err(message) => return StatementEvent::Failed(CliHttp::local(message)),
    };
    match builder.timeout(timeout).json(request).send() {
        Err(error) => StatementEvent::Failed(CliHttp::transport(error)),
        Ok(response) => match crate::client::session::decode::<StatementResult>(response) {
            Ok(result) => match result.error {
                Some(error) => StatementEvent::Failed(CliHttp {
                    status: Some(200),
                    code: None,
                    message: format!("query {} failed: {error}", result.id),
                    timed_out: false,
                    connect: false,
                }),
                None => StatementEvent::Finished(result),
            },
            Err(failure) => StatementEvent::Failed(failure),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session::test_server::{fixture, session};

    #[test]
    fn submit_tags_the_request_and_reports_the_result() {
        let (url, thread) = fixture(vec![(
            "POST /v1/statement ",
            200,
            r#"{"id":"q1","state":"FINISHED","columns":[{"name":"n","type":"Int64"}],"data":[[1]],"elapsed_ms":3}"#.into(),
        )]);
        let (session, mut options) = session(&url);
        options.client_tags = vec!["team:analytics".to_owned()];
        let handle = submit(
            Arc::new(Mutex::new(session)),
            options.server.clone(),
            StatementRequest::new("SELECT 1", &options),
            Duration::from_secs(5),
        );
        assert!(handle.tag.starts_with("kaveon-cli:"));
        match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            StatementEvent::Finished(result) => {
                assert_eq!(result.id, "q1");
                assert_eq!(result.state, "FINISHED");
                assert_eq!(result.column_names(), vec!["n".to_owned()]);
                assert_eq!(result.columns[0].data_type, "Int64");
                assert_eq!(result.data, vec![vec![serde_json::json!(1)]]);
                assert_eq!(result.elapsed_ms, 3);
                assert!(result.next_uri.is_none());
            }
            StatementEvent::Failed(failure) => panic!("{failure:?}"),
        }
        let bodies = thread.join().unwrap();
        let body: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(body["query"], "SELECT 1");
        assert_eq!(body["client"], "kaveon-cli");
        assert_eq!(body["result_delivery"], "paged");
        assert_eq!(
            body["client_tags"],
            serde_json::json!(["team:analytics", handle.tag])
        );
        assert!(body.get("settings").is_none());
    }

    #[test]
    fn a_failed_statement_is_a_failure_with_the_query_id() {
        let (url, thread) = fixture(vec![(
            "POST /v1/statement ",
            200,
            r#"{"id":"q2","state":"FAILED","error":"table 'a.b.c' not found","elapsed_ms":1}"#
                .into(),
        )]);
        let (session, options) = session(&url);
        let handle = submit(
            Arc::new(Mutex::new(session)),
            options.server.clone(),
            StatementRequest::new("SELECT * FROM c", &options),
            Duration::from_secs(5),
        );
        match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            StatementEvent::Failed(failure) => {
                assert_eq!(failure.status, Some(200));
                assert_eq!(failure.message, "query q2 failed: table 'a.b.c' not found");
            }
            StatementEvent::Finished(_) => panic!("a failed statement is not a result"),
        }
        thread.join().unwrap();
    }

    #[test]
    fn an_http_failure_carries_the_coordinator_code() {
        let (url, thread) = fixture(vec![(
            "POST /v1/statement ",
            413,
            r#"{"error":"inline result exceeds 16 MiB","code":"RESULT_TOO_LARGE"}"#.into(),
        )]);
        let (session, options) = session(&url);
        let handle = submit(
            Arc::new(Mutex::new(session)),
            options.server.clone(),
            StatementRequest::new("SELECT *", &options),
            Duration::from_secs(5),
        );
        match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            StatementEvent::Failed(failure) => {
                assert_eq!(failure.status, Some(413));
                assert_eq!(failure.code.as_deref(), Some("RESULT_TOO_LARGE"));
                assert_eq!(failure.message, "inline result exceeds 16 MiB");
            }
            StatementEvent::Finished(_) => panic!("an HTTP failure is not a result"),
        }
        thread.join().unwrap();
    }

    #[test]
    fn tags_are_unique() {
        let a = new_tag();
        let b = new_tag();
        assert_ne!(a, b);
        assert!(a.starts_with("kaveon-cli:") && a.len() > "kaveon-cli:".len());
    }
}
