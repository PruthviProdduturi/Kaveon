//! Paged results: a cursor over `GET /v1/query/{id}/results/{page}`.
//!
//! A statement submitted with `result_delivery: "paged"` answers with a
//! `next_uri`; every page the coordinator returns is
//! `{"id", "data": [[value, …], …], "next_uri": "/v1/query/{id}/results/{n}" | null,
//! "row_count": <rows in the whole result>}` (`server/src/results.rs`).
//! `next_uri` is a server-relative path on today's coordinator; an absolute
//! URI is accepted only when it points at the same origin as the session's
//! server, so a compromised or misconfigured coordinator cannot redirect the
//! bearer token elsewhere.
use crate::auth::Session;
use crate::client::session::{CliHttp, METADATA_TIMEOUT, decode, endpoint};
use serde::Deserialize;
use serde_json::Value;

pub const UNSAFE_NEXT_URI: &str = "coordinator returned an unsafe next URI";
const RESULTS_PREFIX: &str = "/v1/query/";

/// One fetched page. `index` counts pages as this cursor fetched them,
/// starting at zero, independent of the page number in the URI.
#[derive(Debug, Clone)]
pub struct Page {
    pub rows: Vec<Vec<Value>>,
    pub next_uri: Option<String>,
    pub index: usize,
}

#[derive(Deserialize)]
struct PageBody {
    #[serde(default)]
    data: Vec<Vec<Value>>,
    #[serde(default)]
    next_uri: Option<String>,
    #[serde(default)]
    row_count: Option<usize>,
}

/// Fetches pages on demand and keeps the ones it has fetched.
#[derive(Debug)]
pub struct PageCursor {
    server: reqwest::Url,
    /// Absolute URL of the next page, `None` once the result is exhausted.
    next: Option<String>,
    fetched: Vec<Page>,
    rows: usize,
    /// The result's row count as the coordinator reports it on every page;
    /// `None` until the first page arrives.
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

    /// Fetches the next page, `Ok(None)` once the result is exhausted. An
    /// HTTP or transport failure leaves the cursor where it was, so the
    /// same page can be retried; a page carrying an unsafe `next_uri` is
    /// discarded and the cursor is exhausted.
    pub fn fetch_next(&mut self, session: &Session) -> Result<Option<Page>, CliHttp> {
        let Some(url) = self.next.clone() else {
            return Ok(None);
        };
        let response = session
            .request(reqwest::Method::GET, &url)
            .map_err(CliHttp::local)?
            .timeout(METADATA_TIMEOUT)
            .send()
            .map_err(CliHttp::transport)?;
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
        if body.row_count.is_some() {
            self.total_rows = body.row_count;
        }
        let page = Page {
            rows: body.data,
            next_uri: body.next_uri,
            index: self.fetched.len(),
        };
        self.rows += page.rows.len();
        self.next = next;
        self.fetched.push(page.clone());
        Ok(Some(page))
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
    use crate::client::session::test_server::{fixture, session};

    const SERVER: &str = "http://127.0.0.1:8080";

    fn page(rows: &str, next: Option<&str>, total: usize) -> String {
        let next = next.map_or("null".to_owned(), |next| format!("\"{next}\""));
        format!(r#"{{"id":"q","data":{rows},"next_uri":{next},"row_count":{total}}}"#)
    }

    #[test]
    fn two_pages_then_exhaustion() {
        let (url, thread) = fixture(vec![
            (
                "GET /v1/query/q/results/1 ",
                200,
                page(r#"[[1,"a"],[2,"b"]]"#, Some("/v1/query/q/results/2"), 3),
            ),
            (
                "GET /v1/query/q/results/2 ",
                200,
                page(r#"[[3,null]]"#, None, 3),
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

        let first = cursor.fetch_next(&session).unwrap().unwrap();
        assert_eq!(first.index, 0);
        assert_eq!(first.rows.len(), 2);
        assert_eq!(first.rows[1][1], Value::from("b"));
        assert_eq!(first.next_uri.as_deref(), Some("/v1/query/q/results/2"));
        assert_eq!(cursor.rows_so_far(), 2);
        assert_eq!(cursor.total_rows, Some(3));
        assert!(!cursor.exhausted());

        let second = cursor.fetch_next(&session).unwrap().unwrap();
        assert_eq!(second.index, 1);
        assert_eq!(second.rows, vec![vec![Value::from(3), Value::Null]]);
        assert_eq!(second.next_uri, None);
        assert!(cursor.exhausted());
        assert_eq!(cursor.rows_so_far(), 3);

        assert!(cursor.fetch_next(&session).unwrap().is_none());
        assert_eq!(cursor.fetched().len(), 2);
        assert_eq!(cursor.fetched()[0].rows.len(), 2);
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
    fn a_page_with_an_unsafe_next_uri_fails_and_exhausts_the_cursor() {
        let (url, thread) = fixture(vec![(
            "GET /v1/query/q/results/0 ",
            200,
            page(
                r#"[[1]]"#,
                Some("http://evil.example/v1/query/q/results/1"),
                2,
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
