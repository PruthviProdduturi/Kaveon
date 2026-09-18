//! Typed wrappers over the coordinator's `/v1` metadata endpoints.
use crate::auth::Session;
use reqwest::blocking::Response;
use serde::Deserialize;
use std::time::Duration;

pub const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
/// The coordinator drops a worker after 30 s without a heartbeat, so a
/// worker in the payload is live by the server's own rule; this guards only
/// against clock skew between the client and the coordinator.
const STALE_HEARTBEAT_SECS: u64 = 90;

/// A transport or HTTP failure as the coordinator reported it. The error
/// module turns this into a message for people.
#[derive(Debug, Clone)]
pub struct CliHttp {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub timed_out: bool,
    pub connect: bool,
}

impl CliHttp {
    pub fn transport(error: reqwest::Error) -> Self {
        CliHttp {
            status: None,
            code: None,
            message: error.to_string(),
            timed_out: error.is_timeout(),
            connect: error.is_connect(),
        }
    }

    pub fn local(message: impl Into<String>) -> Self {
        CliHttp {
            status: None,
            code: None,
            message: message.into(),
            timed_out: false,
            connect: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    pub node_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub last_heartbeat: u64,
    #[serde(default)]
    pub memory_rss_bytes: u64,
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
    #[serde(default)]
    pub admission: Option<Admission>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Admission {
    #[serde(default)]
    pub limit_bytes: u64,
    #[serde(default)]
    pub admitted_bytes: u64,
    #[serde(default)]
    pub queue_depth: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cluster {
    #[serde(default)]
    pub environment: String,
    pub coordinator: Node,
    #[serde(default)]
    pub workers: Vec<Node>,
}

impl Cluster {
    /// (ready, stale): a worker is ready when its heartbeat is within 30 s
    /// of `now_unix`.
    pub fn ready_workers(&self, now_unix: u64) -> (usize, usize) {
        let ready = self
            .workers
            .iter()
            .filter(|worker| now_unix.saturating_sub(worker.last_heartbeat) <= STALE_HEARTBEAT_SECS)
            .count();
        (ready, self.workers.len() - ready)
    }

    pub fn admission_limit_bytes(&self) -> Option<u64> {
        self.coordinator
            .admission
            .as_ref()
            .map(|admission| admission.limit_bytes)
    }

    pub fn queue_depth(&self) -> u64 {
        self.coordinator
            .admission
            .as_ref()
            .map_or(0, |admission| admission.queue_depth)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Whoami {
    pub principal: String,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub auth: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct QueryRecord {
    pub id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub admission_wait_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub execution: Option<Execution>,
    #[serde(default)]
    pub scan_metrics_complete: Option<bool>,
    #[serde(default)]
    pub stages: Vec<Stage>,
    #[serde(default)]
    pub scans: Vec<Scan>,
    #[serde(default)]
    pub context: Context,
    #[serde(default)]
    pub plan: Option<serde_json::Value>,
    #[serde(default)]
    pub timings: Option<Timings>,
    #[serde(default)]
    pub cached_from: Option<String>,
    /// The result's columns; the coordinator fills them once the statement
    /// is planned, so a running paged statement can be read by its pages.
    #[serde(default)]
    pub columns: Vec<crate::client::statement::Column>,
    /// `/v1/query/{id}/results/0` while a paged statement runs and its
    /// pages can be read; absent for inline delivery and once finished.
    #[serde(default)]
    pub next_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Execution {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// The coordinator's phase timings, microseconds; each absent until the
/// phase ran.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Timings {
    #[serde(default)]
    pub analysis_us: Option<u64>,
    #[serde(default)]
    pub planning_us: Option<u64>,
    #[serde(default)]
    pub execution_us: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Stage {
    #[serde(default)]
    pub stage_id: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub task_count: usize,
    #[serde(default)]
    pub completed_tasks: usize,
    #[serde(default)]
    pub elapsed_us: u64,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Task {
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub partition_index: u64,
    #[serde(default)]
    pub elapsed_us: u64,
    #[serde(default)]
    pub output_rows: u64,
    #[serde(default)]
    pub output_bytes: u64,
    #[serde(default)]
    pub execution: Option<TaskExecution>,
    #[serde(default)]
    pub scan: Option<TaskScan>,
}

/// A task's execution counters, the ones `EXPLAIN ANALYZE` shows.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TaskExecution {
    #[serde(default)]
    pub compute_cpu_us: u64,
    #[serde(default)]
    pub compute_wall_us: u64,
    #[serde(default)]
    pub memory_peak_bytes: u64,
    #[serde(default)]
    pub spill_bytes_written: u64,
    #[serde(default)]
    pub exchange_input_bytes: u64,
    #[serde(default)]
    pub exchange_output_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TaskScan {
    #[serde(default)]
    pub rows_selected: u64,
    #[serde(default)]
    pub rows_emitted: u64,
    #[serde(default)]
    pub compressed_bytes_selected: u64,
    #[serde(default)]
    pub read_ns: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Scan {
    #[serde(default)]
    pub rows_selected: u64,
    #[serde(default)]
    pub rows_emitted: Option<u64>,
    #[serde(default)]
    pub compressed_bytes_selected: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Context {
    #[serde(default)]
    pub client_tags: Vec<String>,
}

pub fn endpoint(server: &str, path: &str) -> String {
    format!("{}{}", server.trim_end_matches('/'), path)
}

pub fn url_with_segments(server: &str, base: &str, segments: &[&str]) -> Result<String, CliHttp> {
    let mut url = reqwest::Url::parse(&endpoint(server, base))
        .map_err(|error| CliHttp::local(format!("invalid coordinator URL: {error}")))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| CliHttp::local("coordinator URL cannot take a path"))?;
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url.into())
}

pub(crate) fn decode<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, CliHttp> {
    let status = response.status();
    let body = response.text().map_err(CliHttp::transport)?;
    if !status.is_success() {
        let value = serde_json::from_str::<serde_json::Value>(&body).ok();
        let message = value
            .as_ref()
            .and_then(|value| value.get("error").and_then(|e| e.as_str()))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if body.trim().is_empty() {
                    status.to_string()
                } else {
                    body.clone()
                }
            });
        let code = value
            .as_ref()
            .and_then(|value| value.get("code").and_then(|c| c.as_str()))
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
        message: format!("invalid coordinator response: {error}"),
        timed_out: false,
        connect: false,
    })
}

pub fn get<T: for<'de> Deserialize<'de>>(session: &Session, url: &str) -> Result<T, CliHttp> {
    let response = session
        .request(reqwest::Method::GET, url)
        .map_err(CliHttp::local)?
        .timeout(METADATA_TIMEOUT)
        .send()
        .map_err(CliHttp::transport)?;
    decode(response)
}

pub fn fetch_cluster(session: &Session, server: &str) -> Result<Cluster, CliHttp> {
    get(session, &endpoint(server, "/v1/cluster"))
}

/// `None` when the coordinator predates `/v1/whoami`.
pub fn fetch_whoami(session: &Session, server: &str) -> Result<Option<Whoami>, CliHttp> {
    match get::<Whoami>(session, &endpoint(server, "/v1/whoami")) {
        Ok(whoami) => Ok(Some(whoami)),
        Err(failure) if failure.status == Some(404) => Ok(None),
        Err(failure) => Err(failure),
    }
}

pub fn fetch_query(session: &Session, server: &str, id: &str) -> Result<QueryRecord, CliHttp> {
    get(session, &url_with_segments(server, "/v1/query", &[id])?)
}

pub fn find_query_by_tag(
    session: &Session,
    server: &str,
    tag: &str,
) -> Result<Option<QueryRecord>, CliHttp> {
    let records: Vec<QueryRecord> = get(session, &endpoint(server, "/v1/query"))?;
    Ok(records
        .into_iter()
        .find(|record| record.context.client_tags.iter().any(|t| t == tag)))
}

pub fn cancel_query(session: &Session, server: &str, id: &str) -> Result<(), CliHttp> {
    let url = url_with_segments(server, "/v1/query", &[id])?;
    let response = session
        .request(reqwest::Method::DELETE, &url)
        .map_err(CliHttp::local)?
        .timeout(METADATA_TIMEOUT)
        .send()
        .map_err(CliHttp::transport)?;
    if response.status().is_success() || response.status().as_u16() == 409 {
        return Ok(());
    }
    decode::<serde_json::Value>(response).map(|_| ())
}

#[cfg(test)]
pub(crate) mod test_server {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One canned response: the request-line prefix it expects, and what
    /// it answers.
    pub(crate) struct Reply {
        pub(crate) expected: &'static str,
        pub(crate) status: u16,
        pub(crate) headers: Vec<(&'static str, String)>,
        pub(crate) body: String,
    }

    /// Serves `responses` in order: (expected request-line prefix, status, body).
    /// Returns the base URL and a handle yielding the request bodies seen.
    pub(crate) fn fixture(
        responses: Vec<(&'static str, u16, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        fixture_with(
            responses
                .into_iter()
                .map(|(expected, status, body)| Reply {
                    expected,
                    status,
                    headers: Vec::new(),
                    body,
                })
                .collect(),
        )
    }

    /// `fixture` with extra response headers per reply.
    pub(crate) fn fixture_with(
        responses: Vec<Reply>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for Reply {
                expected,
                status,
                headers,
                body,
            } in responses
            {
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
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                assert!(
                    line.starts_with(expected),
                    "got {line}, expected {expected}"
                );
                bodies.push(text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned());
                let extra: String = headers
                    .iter()
                    .map(|(name, value)| format!("{name}: {value}\r\n"))
                    .collect();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            bodies
        });
        (url, thread)
    }

    pub(crate) fn session(url: &str) -> (crate::auth::Session, crate::args::Options) {
        let crate::args::Command::Run(mut options) =
            crate::args::parse(&["kaveon".into(), "--auth".into(), "none".into()]).unwrap()
        else {
            panic!("run command expected")
        };
        options.server = url.to_owned();
        (crate::auth::Session::connect(&options).unwrap(), *options)
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{fixture, session};
    use super::*;

    #[test]
    fn cluster_counts_ready_and_stale_workers() {
        let body = r#"{"environment":"docker","coordinator":{"node_id":"c","role":"coordinator","address":"http://c","environment":"docker","version":"0.1.0","last_heartbeat":1000,"admission":{"limit_bytes":4294967296}},"workers":[{"node_id":"w1","role":"worker","address":"http://w1","environment":"docker","version":"0.1.0","last_heartbeat":990},{"node_id":"w2","role":"worker","address":"http://w2","environment":"docker","version":"0.1.0","last_heartbeat":900}]}"#;
        let (url, thread) = fixture(vec![("GET /v1/cluster ", 200, body.into())]);
        let (session, options) = session(&url);
        let cluster = fetch_cluster(&session, &options.server).unwrap();
        assert_eq!(cluster.ready_workers(1000), (1, 1));
        assert_eq!(cluster.admission_limit_bytes(), Some(4294967296));
        assert_eq!(cluster.coordinator.version, "0.1.0");
        thread.join().unwrap();
    }

    #[test]
    fn whoami_is_none_on_404() {
        let (url, thread) = fixture(vec![("GET /v1/whoami ", 404, "{}".into())]);
        let (session, options) = session(&url);
        assert!(fetch_whoami(&session, &options.server).unwrap().is_none());
        thread.join().unwrap();
    }

    #[test]
    fn find_query_by_tag_returns_the_matching_record() {
        let body = r#"[{"id":"q1","state":"RUNNING","elapsed_ms":5,"admission_wait_ms":0,"error":null,"stages":[],"scans":[],"context":{"client_tags":["kaveon-cli:abc"]}},{"id":"q2","state":"FINISHED","elapsed_ms":1,"admission_wait_ms":0,"error":null,"stages":[],"scans":[],"context":{"client_tags":[]}}]"#;
        let (url, thread) = fixture(vec![("GET /v1/query ", 200, body.into())]);
        let (session, options) = session(&url);
        let record = find_query_by_tag(&session, &options.server, "kaveon-cli:abc")
            .unwrap()
            .unwrap();
        assert_eq!(record.id, "q1");
        thread.join().unwrap();
    }

    #[test]
    fn a_running_paged_record_carries_its_columns_and_next_uri() {
        let body = r#"{"id":"q1","state":"RUNNING","elapsed_ms":5,"columns":[{"name":"n","type":"Int64"}],"next_uri":"/v1/query/q1/results/0"}"#;
        let (url, thread) = fixture(vec![("GET /v1/query/q1 ", 200, body.into())]);
        let (session, options) = session(&url);
        let record = fetch_query(&session, &options.server, "q1").unwrap();
        assert_eq!(record.next_uri.as_deref(), Some("/v1/query/q1/results/0"));
        assert_eq!(record.columns.len(), 1);
        assert_eq!(record.columns[0].name, "n");
        assert_eq!(record.columns[0].data_type, "Int64");
        let finished: QueryRecord =
            serde_json::from_str(r#"{"id":"q2","state":"FINISHED"}"#).unwrap();
        assert!(finished.next_uri.is_none() && finished.columns.is_empty());
        thread.join().unwrap();
    }

    #[test]
    fn http_failures_carry_status_and_code() {
        let (url, thread) = fixture(vec![(
            "GET /v1/query/x ",
            404,
            r#"{"error":"query 'x' not found","code":"QUERY_NOT_FOUND"}"#.into(),
        )]);
        let (session, options) = session(&url);
        let failure = fetch_query(&session, &options.server, "x").unwrap_err();
        assert_eq!(failure.status, Some(404));
        assert_eq!(failure.code.as_deref(), Some("QUERY_NOT_FOUND"));
        assert_eq!(failure.message, "query 'x' not found");
        thread.join().unwrap();
    }

    #[test]
    fn cancel_accepts_no_content_and_already_canceled() {
        let (url, thread) = fixture(vec![
            ("DELETE /v1/query/q1 ", 204, String::new()),
            (
                "DELETE /v1/query/q1 ",
                409,
                r#"{"error":"already canceled","code":"QUERY_CANCELED"}"#.into(),
            ),
        ]);
        let (session, options) = session(&url);
        cancel_query(&session, &options.server, "q1").unwrap();
        cancel_query(&session, &options.server, "q1").unwrap();
        thread.join().unwrap();
    }
}
