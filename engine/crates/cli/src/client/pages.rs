//! Paged results: a cursor over `GET /v1/query/{id}/results/{page}`.
//!
//! A statement submitted with `result_delivery: "paged"` answers with a
//! `next_uri`, and while it runs its query record carries one too, so the
//! pages can be read as the coordinator writes them (1,000 rows or 4 MiB
//! each). A written page is
//! `{"id", "data": [[value, …], …], "next_uri": "/v1/query/{id}/results/{n+1}" | null,
//! "row_count": <rows written so far; the total once complete>, "complete": bool}`
//! (`server/src/results.rs`); `next_uri` stays non-null while the writer is
//! in progress even when the next page is not written yet. A page the
//! writer has not reached answers `202 Accepted` with `Retry-After` and
//! `{"id", "row_count", "complete": false}`; a page past the end (or an
//! unknown or expired result) is a 404; a statement that failed or was
//! cancelled mid-read answers 410 (or 404).
//!
//! `next_uri` is a server-relative path on today's coordinator; an absolute
//! URI is accepted only when it points at the same origin as the session's
//! server, so a compromised or misconfigured coordinator cannot redirect the
//! bearer token elsewhere.
use crate::auth::Session;
use crate::client::session::{CliHttp, METADATA_TIMEOUT, decode, endpoint};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub const UNSAFE_NEXT_URI: &str = "coordinator returned an unsafe next URI";
const RESULTS_PREFIX: &str = "/v1/query/";
/// `Retry-After` when a 202 carries none, or an unreadable one.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);

/// One fetched page. `index` counts pages as this cursor fetched them,
/// starting at zero, independent of the page number in the URI.
#[derive(Debug, Clone)]
pub struct Page {
    pub rows: Vec<Vec<Value>>,
    pub next_uri: Option<String>,
    pub index: usize,
    /// Rows the coordinator had written when it answered: the whole
    /// result's count once `complete`.
    pub row_count: usize,
    /// The writer has finished; `row_count` is the total.
    pub complete: bool,
}

/// What one `fetch_next` found.
#[derive(Debug, Clone)]
pub enum Fetched {
    Page(Page),
    /// The writer has not reached this page yet (202): try again after
    /// `retry_after`; `rows_so_far` is what it had written.
    NotYet {
        retry_after: Duration,
        rows_so_far: usize,
    },
    /// Every page has been fetched.
    Exhausted,
}

#[derive(Deserialize)]
struct PageBody {
    #[serde(default)]
    data: Vec<Vec<Value>>,
    #[serde(default)]
    next_uri: Option<String>,
    #[serde(default)]
    row_count: Option<usize>,
    /// Absent on a coordinator that publishes only complete results; its
    /// last page (no `next_uri`) is then the complete one.
    #[serde(default)]
    complete: Option<bool>,
}

/// Fetches pages on demand and keeps the ones it has fetched.
#[derive(Debug)]
pub struct PageCursor {
    server: reqwest::Url,
    /// Absolute URL of the next page, `None` once the result is exhausted.
    next: Option<String>,
    fetched: Vec<Page>,
    rows: usize,
    /// The result's row count, known once a page said the writer was
    /// complete; `None` until then.
    pub total_rows: Option<usize>,
}

impl PageCursor {
    /// Validates `next_uri` against `server` before any request is made.
    pub fn new(server: &str, next_uri: &str) -> Result<PageCursor, CliHttp> {
        let server = reqwest::Url::parse(server.trim_end_matches('/'))
            .map_err(|error| CliHttp::local(format!("invalid coordinator URL: {error}")))?;
        let next = resolve(&server, next_uri)?;
        Ok(PageCursor {
            server,
            next: Some(next),
            fetched: Vec::new(),
            rows: 0,
            total_rows: None,
        })
    }

    /// Fetches the next page with the metadata timeout; see
    /// [`PageCursor::fetch_next_within`].
    pub fn fetch_next(&mut self, session: &Session) -> Result<Fetched, CliHttp> {
        self.fetch_next_within(session, METADATA_TIMEOUT)
    }

    /// Fetches the next page, `Fetched::Exhausted` once the result is,
    /// `Fetched::NotYet` while the coordinator has not written it. An HTTP
    /// or transport failure (a 410 for a statement that failed mid-read
    /// surfaces as `status: Some(410)`) leaves the cursor where it was, so
    /// the same page can be retried; a page carrying an unsafe `next_uri`
    /// is discarded and the cursor is exhausted.
    pub fn fetch_next_within(
        &mut self,
        session: &Session,
        timeout: Duration,
    ) -> Result<Fetched, CliHttp> {
        let Some(url) = self.next.clone() else {
            return Ok(Fetched::Exhausted);
        };
        let response = session
            .request(reqwest::Method::GET, &url)
            .map_err(CliHttp::local)?
            .timeout(timeout)
            .send()
            .map_err(CliHttp::transport)?;
        if response.status() == reqwest::StatusCode::ACCEPTED {
            let retry_after = retry_after(response.headers());
            let body: PageBody = decode(response)?;
            return Ok(Fetched::NotYet {
                retry_after,
                rows_so_far: body.row_count.unwrap_or(0),
            });
        }
        let body: PageBody = decode(response)?;
        let next = match &body.next_uri {
            Some(next_uri) => match resolve(&self.server, next_uri) {
                Ok(next) => Some(next),
                Err(failure) => {
                    self.next = None;
                    return Err(failure);
                }
            },
            None => None,
        };
        let complete = body.complete.unwrap_or(body.next_uri.is_none());
        let row_count = body.row_count.unwrap_or(0);
        if complete {
            self.total_rows = Some(row_count);
        }
        let page = Page {
            rows: body.data,
            next_uri: body.next_uri,
            index: self.fetched.len(),
            row_count,
            complete,
        };
        self.rows += page.rows.len();
        self.next = next;
        self.fetched.push(page.clone());
        Ok(Fetched::Page(page))
    }

    pub fn fetched(&self) -> &[Page] {
        &self.fetched
    }

    pub fn rows_so_far(&self) -> usize {
        self.rows
    }

    pub fn exhausted(&self) -> bool {
        self.next.is_none()
    }

    /// The absolute URL the next `fetch_next` will request.
    pub fn next_url(&self) -> Option<&str> {
        self.next.as_deref()
    }
}

/// `Retry-After` in seconds (the delay form; a date is not expected from
/// the coordinator), `DEFAULT_RETRY_AFTER` when absent or unreadable.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_AFTER, Duration::from_secs)
}

/// Turns a `next_uri` into an absolute URL on the session's server, or
/// refuses it. A server-relative path is joined to the server's origin and
/// base path; an absolute URI must share the server's scheme, host and port
/// and carry no userinfo or fragment. The path must sit under the server's
/// base path at `/v1/query/` after normalisation.
fn resolve(server: &reqwest::Url, next_uri: &str) -> Result<String, CliHttp> {
    let unsafe_uri = || CliHttp::local(UNSAFE_NEXT_URI);
    let candidate = if next_uri.starts_with('/') {
        reqwest::Url::parse(&endpoint(server.as_str(), next_uri)).map_err(|_| unsafe_uri())?
    } else {
        reqwest::Url::parse(next_uri).map_err(|_| unsafe_uri())?
    };
    let same_origin = candidate.scheme() == server.scheme()
        && candidate.host_str() == server.host_str()
        && candidate.port_or_known_default() == server.port_or_known_default();
    let clean = candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.fragment().is_none();
    let base = server.path().trim_end_matches('/');
    let under_results = candidate
        .path()
        .strip_prefix(base)
        .is_some_and(|rest| rest.starts_with(RESULTS_PREFIX));
    if same_origin && clean && under_results {
        Ok(candidate.into())
    } else {
        Err(unsafe_uri())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session::test_server::{Reply, fixture, fixture_with, session};

    const SERVER: &str = "http://127.0.0.1:8080";

    fn page(rows: &str, next: Option<&str>, count: usize, complete: bool) -> String {
        let next = next.map_or("null".to_owned(), |next| format!("\"{next}\""));
        format!(
            r#"{{"id":"q","data":{rows},"next_uri":{next},"row_count":{count},"complete":{complete}}}"#
        )
    }

    fn expect_page(fetched: Result<Fetched, CliHttp>) -> Page {
        match fetched {
            Ok(Fetched::Page(page)) => page,
            other => panic!("expected a page, got {other:?}"),
        }
    }

    #[test]
    fn two_pages_then_exhaustion() {
        let (url, thread) = fixture(vec![
            (
                "GET /v1/query/q/results/1 ",
                200,
                page(
                    r#"[[1,"a"],[2,"b"]]"#,
                    Some("/v1/query/q/results/2"),
                    2,
                    false,
                ),
            ),
            (
                "GET /v1/query/q/results/2 ",
                200,
                page(r#"[[3,null]]"#, None, 3, true),
            ),
        ]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/1").unwrap();
        assert_eq!(
            cursor.next_url(),
            Some(format!("{url}/v1/query/q/results/1").as_str())
        );
        assert!(!cursor.exhausted());
        assert_eq!(cursor.total_rows, None);

        let first = expect_page(cursor.fetch_next(&session));
        assert_eq!(first.index, 0);
        assert_eq!(first.rows.len(), 2);
        assert_eq!(first.rows[1][1], Value::from("b"));
        assert_eq!(first.next_uri.as_deref(), Some("/v1/query/q/results/2"));
        assert_eq!(first.row_count, 2);
        assert!(!first.complete);
        assert_eq!(cursor.rows_so_far(), 2);
        // Rows so far, not a total: the writer was still going.
        assert_eq!(cursor.total_rows, None);
        assert!(!cursor.exhausted());

        let second = expect_page(cursor.fetch_next(&session));
        assert_eq!(second.index, 1);
        assert_eq!(second.rows, vec![vec![Value::from(3), Value::Null]]);
        assert_eq!(second.next_uri, None);
        assert!(second.complete);
        assert_eq!(second.row_count, 3);
        assert!(cursor.exhausted());
        assert_eq!(cursor.rows_so_far(), 3);
        assert_eq!(cursor.total_rows, Some(3));

        assert!(matches!(
            cursor.fetch_next(&session).unwrap(),
            Fetched::Exhausted
        ));
        assert_eq!(cursor.fetched().len(), 2);
        assert_eq!(cursor.fetched()[0].rows.len(), 2);
        thread.join().unwrap();
    }

    #[test]
    fn a_complete_page_with_more_pages_sets_the_total() {
        // The writer finished while the reader was on page 0: the page is
        // complete, the total is known, and page 1 is still to fetch.
        let (url, thread) = fixture(vec![(
            "GET /v1/query/q/results/0 ",
            200,
            page(r#"[[1]]"#, Some("/v1/query/q/results/1"), 1_500, true),
        )]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/0").unwrap();
        let first = expect_page(cursor.fetch_next(&session));
        assert!(first.complete);
        assert_eq!(cursor.total_rows, Some(1_500));
        assert!(!cursor.exhausted());
        thread.join().unwrap();
    }

    #[test]
    fn a_coordinator_without_complete_is_complete_on_its_last_page() {
        let (url, thread) = fixture(vec![
            (
                "GET /v1/query/q/results/0 ",
                200,
                r#"{"id":"q","data":[[1]],"next_uri":"/v1/query/q/results/1","row_count":2}"#
                    .to_owned(),
            ),
            (
                "GET /v1/query/q/results/1 ",
                200,
                r#"{"id":"q","data":[[2]],"next_uri":null,"row_count":2}"#.to_owned(),
            ),
        ]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/0").unwrap();
        assert!(!expect_page(cursor.fetch_next(&session)).complete);
        assert_eq!(cursor.total_rows, None);
        assert!(expect_page(cursor.fetch_next(&session)).complete);
        assert_eq!(cursor.total_rows, Some(2));
        thread.join().unwrap();
    }

    #[test]
    fn a_page_not_written_yet_says_when_to_retry_and_keeps_the_cursor() {
        let (url, thread) = fixture_with(vec![
            Reply {
                expected: "GET /v1/query/q/results/0 ",
                status: 202,
                headers: vec![("Retry-After", "3".to_owned())],
                body: r#"{"id":"q","row_count":12000,"complete":false}"#.to_owned(),
            },
            Reply {
                expected: "GET /v1/query/q/results/0 ",
                status: 202,
                headers: Vec::new(),
                body: r#"{"id":"q","row_count":12500,"complete":false}"#.to_owned(),
            },
            Reply {
                expected: "GET /v1/query/q/results/0 ",
                status: 200,
                headers: Vec::new(),
                body: page(r#"[[1]]"#, Some("/v1/query/q/results/1"), 13_000, false),
            },
        ]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/0").unwrap();
        match cursor
            .fetch_next_within(&session, Duration::from_secs(5))
            .unwrap()
        {
            Fetched::NotYet {
                retry_after,
                rows_so_far,
            } => {
                assert_eq!(retry_after, Duration::from_secs(3));
                assert_eq!(rows_so_far, 12_000);
            }
            other => panic!("expected NotYet, got {other:?}"),
        }
        assert_eq!(
            cursor.next_url(),
            Some(format!("{url}/v1/query/q/results/0").as_str())
        );
        assert!(cursor.fetched().is_empty());
        assert_eq!(cursor.total_rows, None);
        // No Retry-After: a second by default.
        match cursor.fetch_next(&session).unwrap() {
            Fetched::NotYet {
                retry_after,
                rows_so_far,
            } => {
                assert_eq!(retry_after, DEFAULT_RETRY_AFTER);
                assert_eq!(rows_so_far, 12_500);
            }
            other => panic!("expected NotYet, got {other:?}"),
        }
        let page = expect_page(cursor.fetch_next(&session));
        assert_eq!(page.index, 0);
        assert_eq!(page.row_count, 13_000);
        assert_eq!(cursor.rows_so_far(), 1);
        thread.join().unwrap();
    }

    #[test]
    fn same_origin_absolute_uri_is_accepted() {
        let cursor = PageCursor::new(SERVER, "http://127.0.0.1:8080/v1/query/q/results/0").unwrap();
        assert_eq!(
            cursor.next_url(),
            Some("http://127.0.0.1:8080/v1/query/q/results/0")
        );
        let cursor = PageCursor::new("http://example.com/", "/v1/query/q/results/0").unwrap();
        assert_eq!(
            cursor.next_url(),
            Some("http://example.com/v1/query/q/results/0")
        );
        // The default port is the same origin whether or not it is spelled out.
        assert!(
            PageCursor::new(
                "http://example.com:80",
                "http://example.com/v1/query/q/results/0"
            )
            .is_ok()
        );
        // A server with a base path keeps it in front of the results route.
        let cursor =
            PageCursor::new("http://example.com/engine/", "/v1/query/q/results/0").unwrap();
        assert_eq!(
            cursor.next_url(),
            Some("http://example.com/engine/v1/query/q/results/0")
        );
    }

    #[test]
    fn unsafe_next_uris_are_refused() {
        let cases = [
            (
                "other host",
                "http://evil.example:8080/v1/query/q/results/0",
            ),
            ("other port", "http://127.0.0.1:9090/v1/query/q/results/0"),
            (
                "other scheme",
                "https://127.0.0.1:8080/v1/query/q/results/0",
            ),
            (
                "userinfo",
                "http://user@127.0.0.1:8080/v1/query/q/results/0",
            ),
            (
                "password",
                "http://user:secret@127.0.0.1:8080/v1/query/q/results/0",
            ),
            ("fragment", "/v1/query/q/results/0#frag"),
            ("wrong path", "/v1/cluster"),
            ("wrong absolute path", "http://127.0.0.1:8080/v1/catalog"),
            ("dot segments", "/v1/query/../../v1/cluster"),
            ("scheme-relative", "//evil.example/v1/query/q/results/0"),
            ("no leading slash", "v1/query/q/results/0"),
            ("empty", ""),
        ];
        for (name, uri) in cases {
            let failure = PageCursor::new(SERVER, uri).expect_err(name);
            assert_eq!(failure.message, UNSAFE_NEXT_URI, "{name}");
            assert_eq!(failure.status, None, "{name}");
        }
        // With a base path the results route must stay under it.
        for uri in [
            "http://example.com/v1/query/q/results/0",
            "/../v1/query/q/results/0",
        ] {
            let failure = PageCursor::new("http://example.com/engine", uri).unwrap_err();
            assert_eq!(failure.message, UNSAFE_NEXT_URI, "{uri}");
        }
    }

    #[test]
    fn expired_page_surfaces_the_404_and_keeps_the_cursor() {
        let (url, thread) = fixture(vec![("GET /v1/query/q/results/3 ", 404, String::new())]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/3").unwrap();
        let failure = cursor.fetch_next(&session).unwrap_err();
        assert_eq!(failure.status, Some(404));
        assert!(!cursor.exhausted());
        assert_eq!(cursor.rows_so_far(), 0);
        assert!(cursor.fetched().is_empty());
        thread.join().unwrap();
    }

    #[test]
    fn a_statement_gone_mid_read_surfaces_the_410() {
        let (url, thread) = fixture(vec![(
            "GET /v1/query/q/results/2 ",
            410,
            r#"{"error":"query q failed: division by zero","code":"QUERY_FAILED"}"#.to_owned(),
        )]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/2").unwrap();
        let failure = cursor.fetch_next(&session).unwrap_err();
        assert_eq!(failure.status, Some(410));
        assert_eq!(failure.code.as_deref(), Some("QUERY_FAILED"));
        assert!(!cursor.exhausted());
        thread.join().unwrap();
    }

    #[test]
    fn a_page_with_an_unsafe_next_uri_fails_and_exhausts_the_cursor() {
        let (url, thread) = fixture(vec![(
            "GET /v1/query/q/results/0 ",
            200,
            page(
                r#"[[1]]"#,
                Some("http://evil.example/v1/query/q/results/1"),
                2,
                false,
            ),
        )]);
        let (session, options) = session(&url);
        let mut cursor = PageCursor::new(&options.server, "/v1/query/q/results/0").unwrap();
        let failure = cursor.fetch_next(&session).unwrap_err();
        assert_eq!(failure.message, UNSAFE_NEXT_URI);
        assert!(cursor.exhausted());
        assert!(cursor.fetched().is_empty());
        thread.join().unwrap();
    }

    #[test]
    fn invalid_server_is_reported() {
        let failure = PageCursor::new("not a url", "/v1/query/q/results/0").unwrap_err();
        assert!(failure.message.starts_with("invalid coordinator URL"));
    }
}
