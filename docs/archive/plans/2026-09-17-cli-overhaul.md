# Kaveon CLI Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the `kaveon` client's interactive experience — informative header, pinned SQL editor with status bar, live running line with cancel, paged results, readable summaries and errors — while keeping every non-interactive output byte-identical to 0.2.0.

**Architecture:** Two threads and one channel: the UI thread runs a ratatui inline viewport (scrollback stays normal; the editor and status bar are pinned at the bottom) and a worker thread performs the blocking `POST /v1/statement`. The UI thread finds the statement's record by a unique client tag and polls it for state. Renderers are split into `plain()` (today's bytes) and `styled()` (TTY). Three additive server endpoints/fields are added in `kaveon-server` while Codex is away (architect's authorization).

**Tech Stack:** Rust 2024 (workspace `rust-version` 1.88), `reqwest::blocking`, `ratatui = "0.29"`, `crossterm = "0.28"`, `tui-textarea = "0.7"`, `unicode-width = "0.2"`, `uuid` (workspace), `serde_json`, `sqlparser` (workspace, tokenizer only). `rustyline` and `terminal_size` are removed.

**Spec:** `docs/superpowers/specs/2026-09-17-cli-overhaul-design.md`

## Global Constraints

- Product name in the client is **Kaveon**, one word: no "Kaveon CLI", "KaveonDB", "Kaveon Engine" in headers, prompts or messages.
- No emoji in UI. Spinner glyphs are braille (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`); status glyphs are `✓` and `✗`.
- Non-TTY (`-e`, `-f`, piped stdin, redirected stdout) prints no header, no colour, no spinner; output is byte-identical to 0.2.0. `NO_COLOR` or `TERM=dumb` → plain rendering even on a TTY.
- No debug logging, no lowered thresholds, root-cause fixes only. Professional register in all copy.
- The client never opens customer storage or executes operators except behind `--local`.
- Gates before every merge to `dev`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (run from `engine/`).
- Work happens on branch `cli-overhaul` in a worktree; rebase onto `dev` before merging (another session merges engine work to `dev` continuously). Commits: no `Co-Authored-By`.
- Version: `engine/crates/cli/Cargo.toml` → `0.3.0` (Task 1).
- Existing tests in `output.rs`, `input.rs`, `args.rs`, `auth.rs` and the metadata parser stay green throughout; move them with their code, never delete.

---

## File structure (end state of `engine/crates/cli/src`)

| File | Responsibility |
|---|---|
| `main.rs` | parse args → `auth::Session::connect` → dispatch: `batch::run` (non-TTY or `-e`/`-f`), `shell::run` (TTY), `local::run` (`--local`) |
| `args.rs` | unchanged parser plus `--paged`, `--no-header`, `--theme`, `--width`; `--timeout` default 24 h; `--disable-auto-suggestion` accepted as a no-op |
| `auth.rs` | unchanged |
| `input.rs` | statement splitter only (rustyline `TerminalInput`/`SqlHelper` removed) |
| `theme.rs` | `Theme { accent, dim, warning, error, ok }` as `ratatui::style::Style`; `Theme::detect(flag, is_tty)` |
| `client/mod.rs` | re-exports |
| `client/session.rs` | `Cluster`, `Whoami`, `QueryRecord` types + `fetch_cluster`, `fetch_whoami`, `fetch_query`, `find_query_by_tag`, `cancel_query`, catalog listing (moved from `remote.rs`) |
| `client/statement.rs` | `Statement::submit(...) -> Handle`; `StatementEvent`; worker thread; result type `StatementResult { id, state, columns, rows, elapsed_ms, next_uri }` |
| `client/pages.rs` | `PageCursor` over `/v1/query/{id}/results/{n}` |
| `client/metadata.rs` | SHOW/USE/DESCRIBE parser (`MetaCommand`) + `NameCache` for completion (moved from `remote.rs`) |
| `client/error.rs` | `CliError { kind: ErrorKind, message, query_id, workers, position }` + `From<HttpFailure>`; de-dup and unwrap |
| `render/mod.rs` | `Format` enum (was `OutputFormat`), `render_plain(...) -> String`, `render_styled(...) -> ratatui::text::Text` |
| `render/table.rs`, `vertical.rs`, `markdown.rs`, `delimited.rs`, `json.rs` | one format each; `plain` functions are the moved bodies of `output.rs` |
| `render/summary.rs` | `Summary` → plain line(s) / styled lines |
| `render/error.rs` | `CliError` → plain line / styled panel |
| `render/plan.rs` | `PlanNode` JSON → indented tree |
| `render/cluster.rs` | header lines and `.cluster` panel from `Cluster` + `Whoami` |
| `shell/mod.rs` | `run(session, options) -> Result<(), String>` |
| `shell/app.rs` | `App` state machine + event loop |
| `shell/editor.rs` | `Editor` wrapping `tui_textarea::TextArea`: highlighting, submit rule, history |
| `shell/status.rs` | box title + bottom status line widgets |
| `shell/progress.rs` | running line widget from `Progress` state |
| `shell/complete.rs` | completion candidates and popup |
| `shell/commands.rs` | dot commands (`.cluster`, `.settings`, `.format`, `.history`, `.help`, `.clear`, `.quit`) |
| `batch.rs` | `-e`/`-f`/stdin execution (moved `execute_script`), plain renderers, `--paged` |
| `local/mod.rs`, `local/planner.rs`, `local/config.rs`, `local/catalog.rs` | embedded engine (moved `planner.rs`, `config.rs`, `build_local_catalog`) producing `StatementResult` |

`remote.rs`, `output.rs`, `display.rs` are deleted by the end of Stage 3.

Server (`engine/crates/server/src`): `api.rs` gains `whoami`, live counters in the task-completion path, `position` on parse errors; `security.rs` gains an `AuthSource` extension. `docs/reference/api.md` gains the entries.

---

## Stage 1 — Branch, whoami, header, inline shell (the intro the architect tests first)

### Task 1: Worktree, branch, dependencies, version

**Files:**
- Modify: `engine/crates/cli/Cargo.toml`

- [ ] **Step 1: Create the worktree and branch**

```bash
cd D:/Repos/PruthviProdduturi/Kaveon
git worktree add .claude/worktrees/cli-overhaul -b cli-overhaul dev
cd .claude/worktrees/cli-overhaul/engine
```

All later steps run from that worktree's `engine/` directory.

- [ ] **Step 2: Update `Cargo.toml`**

Replace the `[package]` version and the dependency block:

```toml
[package]
name = "kaveon-cli"
description = "Kaveon client — interactive SQL shell for a Kaveon coordinator"
version = "0.3.0"
rust-version = "1.88"
edition.workspace = true
license.workspace = true

[[bin]]
name = "kaveon"
path = "src/main.rs"

[dependencies]
kaveon-core = { path = "../core" }
kaveon-exec = { path = "../exec" }
kaveon-optim = { path = "../optim" }
kaveon-sql = { path = "../sql" }
kaveon-storage = { path = "../storage" }
arrow = { workspace = true }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
serde = { workspace = true }
serde_json = { workspace = true }
sqlparser = { workspace = true }
uuid = { workspace = true }
ratatui = "0.29"
crossterm = "0.28"
tui-textarea = "0.7"
unicode-width = "0.2"
```

`rustyline` and `terminal_size` stay until Task 6 removes their users (the crate must keep compiling at every commit). Keep them in this step; delete in Task 6.

- [ ] **Step 3: Build**

Run: `cargo build -p kaveon-cli`
Expected: success (new crates download; nothing uses them yet).

- [ ] **Step 4: Commit**

```bash
git add crates/cli/Cargo.toml Cargo.lock
git commit -m "cli: 0.3.0 — add the terminal UI dependencies"
```

### Task 2: Server — `GET /v1/whoami`

**Files:**
- Modify: `engine/crates/server/src/security.rs` (add `AuthSource`, insert it in `authorize`)
- Modify: `engine/crates/server/src/api.rs` (route + handler + test)
- Modify: `docs/reference/api.md` (entry)

**Interfaces:**
- Produces: `GET /v1/whoami` → `200 {"principal": String, "display": String|null, "role": "reader"|"analyst"|"admin", "auth": "static"|"bridge"|"entra"|"development"|"catalog"|"internal"}`; requires authentication like every other `/v1` route.

- [ ] **Step 1: Write the failing test** in `api.rs` `mod tests` (follow the pattern of the existing router tests that build `AppState` with `test_state()` — search for `fn test_state` and copy its usage; the exact helper name in the file at the time of writing is `crate::test_support::state()` or a local `state()`; use whichever the neighbouring tests use):

```rust
#[tokio::test]
async fn whoami_reports_the_authenticated_identity_and_source() {
    let state = test_state_with_principal("analyst-token-0123456789abcdef0123", "ana", Role::Analyst);
    let app = build_router(state);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/whoami")
                .header("authorization", "Bearer analyst-token-0123456789abcdef0123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(body["principal"], "ana");
    assert_eq!(body["role"], "analyst");
    assert_eq!(body["auth"], "static");
    assert!(body["display"].is_null());
}
```

If no helper builds a state with a static principal, write `test_state_with_principal` next to the test: clone the file's existing state helper and set `config.security.principals = vec![PrincipalCredential { token, principal, role }]`.

- [ ] **Step 2: Run it**

Run: `cargo test -p kaveon-server whoami_reports -- --nocapture`
Expected: FAIL (404 — route missing).

- [ ] **Step 3: Add `AuthSource` to `security.rs`**

```rust
/// How the request was authenticated. Inserted next to `Identity` so
/// handlers can report it without widening `Identity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthSource {
    Static,
    Bridge,
    Entra,
    Development,
    Catalog,
    Internal,
}
```

Change `SecurityConfig::authenticate` to return `Result<(Identity, AuthSource), StatusCode>`: the bridge branch returns `AuthSource::Bridge`, the principals loop `AuthSource::Static`, the insecure-development branch `AuthSource::Development`. In `authorize`, insert the source alongside the identity:

```rust
    let (identity, source) = match state.config.security.authenticate(request.headers()) {
        Ok(pair) => pair,
        Err(status) => {
            let Some(entra) = &state.config.security.entra else {
                return status.into_response();
            };
            let Some(token) = bearer(request.headers()) else {
                return status.into_response();
            };
            match entra.authenticate(token).await {
                Ok(identity) => (identity, AuthSource::Entra),
                Err(status) => return status.into_response(),
            }
        }
    };
    if (path == "/v1/statement" || path.starts_with("/v1/transaction"))
        && identity.role == Role::Reader
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(identity);
    request.extensions_mut().insert(source);
    next.run(request).await
```

The `internal` branch inserts `AuthSource::Internal` next to its identity. The catalog-admin-token branch (`/v1/catalog/` with the catalog token) currently inserts no identity; leave it — `/v1/whoami` is not under `/v1/catalog/`.

Update the two existing callers of `authenticate` in `security.rs` tests (they destructure the identity; add `.0`).

- [ ] **Step 4: Add the handler and route in `api.rs`**

Route, next to `/v1/capabilities`:

```rust
        .route("/v1/whoami", get(whoami))
```

Handler:

```rust
#[derive(Serialize)]
struct WhoamiResponse<'a> {
    principal: &'a str,
    display: Option<&'a str>,
    role: &'static str,
    auth: crate::security::AuthSource,
}

async fn whoami(
    Extension(identity): Extension<Identity>,
    source: Option<Extension<crate::security::AuthSource>>,
) -> Json<serde_json::Value> {
    let role = match identity.role {
        crate::security::Role::Reader => "reader",
        crate::security::Role::Analyst => "analyst",
        crate::security::Role::Admin => "admin",
    };
    let auth = source.map_or(crate::security::AuthSource::Static, |Extension(s)| s);
    Json(serde_json::to_value(WhoamiResponse {
        principal: &identity.principal,
        display: identity.display_identity.as_deref(),
        role,
        auth,
    })
    .expect("whoami serializes"))
}
```

- [ ] **Step 5: Run the server tests**

Run: `cargo test -p kaveon-server`
Expected: all pass, including the new test.

- [ ] **Step 6: Document** — add to `docs/reference/api.md` under the operational endpoints:

```markdown
### `GET /v1/whoami`

Returns the identity the security layer attached to the request:
`principal`, `display` (null unless a validated sign-in supplied one),
`role` (`reader`, `analyst`, `admin`) and `auth` (`static`, `bridge`,
`entra`, `development`, `internal`). The client uses it for the session
header; older coordinators answer 404 and the client hides the line.
```

- [ ] **Step 7: Gates and commit**

Run: `cargo fmt --all -- --check && cargo clippy -p kaveon-server --all-targets -- -D warnings`

```bash
git add crates/server/src/security.rs crates/server/src/api.rs ../docs/reference/api.md
git commit -m "server: GET /v1/whoami reports the authenticated principal, role and source"
```

### Task 3: `theme.rs` and `client/session.rs` (cluster + whoami + query record)

**Files:**
- Create: `engine/crates/cli/src/theme.rs`
- Create: `engine/crates/cli/src/client/mod.rs`, `engine/crates/cli/src/client/session.rs`
- Modify: `engine/crates/cli/src/main.rs` (add `mod client; mod theme;`)

**Interfaces:**
- Produces:
  - `theme::Theme { accent, dim, warning, error, ok, plain: bool }`, `Theme::detect(flag: &str, stdout_is_tty: bool) -> Theme`.
  - `client::session::Cluster { environment: String, coordinator: Node, workers: Vec<Node>, admission_limit_bytes: Option<u64> }`, `Node { node_id, address, version, environment, last_heartbeat: u64 }`, `Cluster::ready_workers(now_unix: u64) -> (usize, usize)` (ready, stale; stale = heartbeat older than 30 s).
  - `Whoami { principal, display: Option<String>, role, auth }`.
  - `QueryRecord { id, state, elapsed_ms, admission_wait_ms, error: Option<String>, execution: Option<Execution>, stages: Vec<Stage>, scans: Vec<Scan>, context: Context, plan: Option<serde_json::Value> }`, `Execution { mode, detail }`, `Stage { task_count, completed_tasks, tasks: Vec<Task> }`, `Task { node_id }`, `Scan { rows_selected, rows_emitted: Option<u64>, compressed_bytes_selected }`, `Context { client_tags: Vec<String> }`.
  - `fn fetch_cluster(session: &Session, server: &str) -> Result<Cluster, CliHttp>`, `fn fetch_whoami(...) -> Result<Option<Whoami>, CliHttp>` (None on 404), `fn fetch_query(session, server, id) -> Result<QueryRecord, CliHttp>`, `fn find_query_by_tag(session, server, tag) -> Result<Option<QueryRecord>, CliHttp>`, `fn cancel_query(session, server, id) -> Result<(), CliHttp>`.
  - `CliHttp { status: Option<u16>, code: Option<String>, message: String, timed_out: bool, connect: bool }` — the raw transport failure that `client/error.rs` (Task 9) turns into `CliError`.

- [ ] **Step 1: Write `theme.rs`**

```rust
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Style,
    pub dim: Style,
    pub warning: Style,
    pub error: Style,
    pub ok: Style,
    pub title: Style,
    /// No colour or box drawing: `mono`, `NO_COLOR`, `TERM=dumb`, or not a TTY.
    pub plain: bool,
}

impl Theme {
    pub fn detect(flag: &str, stdout_is_tty: bool) -> Theme {
        let plain = !stdout_is_tty
            || flag == "mono"
            || std::env::var_os("NO_COLOR").is_some()
            || std::env::var("TERM").is_ok_and(|term| term == "dumb");
        if plain {
            return Theme::mono();
        }
        let accent = if flag == "light" { Color::Blue } else { Color::Cyan };
        Theme {
            accent: Style::default().fg(accent),
            dim: Style::default().fg(Color::DarkGray),
            warning: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            ok: Style::default().fg(Color::Green),
            title: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            plain: false,
        }
    }

    pub fn mono() -> Theme {
        let none = Style::default();
        Theme { accent: none, dim: none, warning: none, error: none, ok: none, title: none, plain: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_tty_and_no_color_are_plain() {
        assert!(Theme::detect("dark", false).plain);
        assert!(Theme::detect("mono", true).plain);
        assert!(!Theme::mono().accent.fg.is_some());
    }
}
```

(`NO_COLOR` is read from the process environment; the test does not set it, to stay independent of the developer's shell.)

- [ ] **Step 2: Write `client/mod.rs`**

```rust
pub mod session;
```

- [ ] **Step 3: Write the failing tests for `client/session.rs`** — a TcpListener fixture like the ones in `remote.rs`. Put this helper in `client/session.rs` `mod tests` (Task 5 moves it to `client/test_server.rs` when a second module needs it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves `responses` in order: (expected request line prefix, status, body).
    pub(crate) fn fixture(responses: Vec<(&'static str, u16, String)>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            for (expected, status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap();
                let line = std::str::from_utf8(&buf[..n]).unwrap().lines().next().unwrap().to_owned();
                assert!(line.starts_with(expected), "got {line}, expected {expected}");
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (url, thread)
    }

    fn session(url: &str) -> (crate::auth::Session, crate::args::Options) {
        let crate::args::Command::Run(mut options) =
            crate::args::parse(&["kaveon".into(), "--auth".into(), "none".into()]).unwrap()
        else { panic!() };
        options.server = url.to_owned();
        (crate::auth::Session::connect(&options).unwrap(), *options)
    }

    #[test]
    fn cluster_counts_ready_and_stale_workers() {
        let body = r#"{"environment":"docker","coordinator":{"node_id":"c","role":"coordinator","address":"http://c","environment":"docker","version":"0.1.0","last_heartbeat":1000,"admission":{"limit_bytes":4294967296}},"workers":[{"node_id":"w1","role":"worker","address":"http://w1","environment":"docker","version":"0.1.0","last_heartbeat":990},{"node_id":"w2","role":"worker","address":"http://w2","environment":"docker","version":"0.1.0","last_heartbeat":900}]}"#;
        let (url, thread) = fixture(vec![("GET /v1/cluster ", 200, body.into())]);
        let (session, options) = session(&url);
        let cluster = fetch_cluster(&session, &options.server).unwrap();
        assert_eq!(cluster.ready_workers(1000), (1, 1));
        assert_eq!(cluster.admission_limit_bytes, Some(4294967296));
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
        let record = find_query_by_tag(&session, &options.server, "kaveon-cli:abc").unwrap().unwrap();
        assert_eq!(record.id, "q1");
        thread.join().unwrap();
    }

    #[test]
    fn http_failures_carry_status_and_code() {
        let (url, thread) = fixture(vec![("GET /v1/query/x ", 404, r#"{"error":"query 'x' not found","code":"QUERY_NOT_FOUND"}"#.into())]);
        let (session, options) = session(&url);
        let failure = fetch_query(&session, &options.server, "x").unwrap_err();
        assert_eq!(failure.status, Some(404));
        assert_eq!(failure.code.as_deref(), Some("QUERY_NOT_FOUND"));
        assert_eq!(failure.message, "query 'x' not found");
        thread.join().unwrap();
    }
}
```

- [ ] **Step 4: Run them**

Run: `cargo test -p kaveon-cli client::session`
Expected: FAIL to compile (types missing).

- [ ] **Step 5: Implement `client/session.rs`**

```rust
//! Typed wrappers over the coordinator's `/v1` metadata endpoints.
use crate::auth::Session;
use reqwest::blocking::Response;
use serde::Deserialize;
use std::time::Duration;

pub const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const STALE_HEARTBEAT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct CliHttp {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub timed_out: bool,
    pub connect: bool,
}

impl CliHttp {
    fn transport(error: reqwest::Error) -> Self {
        CliHttp {
            status: None,
            code: None,
            message: error.to_string(),
            timed_out: error.is_timeout(),
            connect: error.is_connect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    pub node_id: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub last_heartbeat: u64,
    #[serde(default)]
    pub admission: Option<Admission>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Admission {
    #[serde(default)]
    pub limit_bytes: u64,
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
    /// (ready, stale): a worker is ready when its heartbeat is within 30 s of `now_unix`.
    pub fn ready_workers(&self, now_unix: u64) -> (usize, usize) {
        let ready = self
            .workers
            .iter()
            .filter(|w| now_unix.saturating_sub(w.last_heartbeat) <= STALE_HEARTBEAT_SECS)
            .count();
        (ready, self.workers.len() - ready)
    }

    pub fn admission_limit_bytes(&self) -> Option<u64> {
        self.coordinator.admission.as_ref().map(|a| a.limit_bytes)
    }

    pub fn queue_depth(&self) -> u64 {
        self.coordinator.admission.as_ref().map_or(0, |a| a.queue_depth)
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
    pub cached_from: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Execution {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Stage {
    #[serde(default)]
    pub task_count: usize,
    #[serde(default)]
    pub completed_tasks: usize,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Task {
    #[serde(default)]
    pub node_id: String,
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

fn endpoint(server: &str, path: &str) -> String {
    format!("{}{}", server.trim_end_matches('/'), path)
}

fn url_with_segments(server: &str, base: &str, segments: &[&str]) -> Result<String, CliHttp> {
    let mut url = reqwest::Url::parse(&endpoint(server, base)).map_err(|e| CliHttp {
        status: None, code: None, message: format!("invalid coordinator URL: {e}"), timed_out: false, connect: false,
    })?;
    {
        let mut path = url.path_segments_mut().map_err(|_| CliHttp {
            status: None, code: None, message: "coordinator URL cannot take a path".into(), timed_out: false, connect: false,
        })?;
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url.into())
}

fn decode<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, CliHttp> {
    let status = response.status();
    let body = response.text().map_err(CliHttp::transport)?;
    if !status.is_success() {
        let value = serde_json::from_str::<serde_json::Value>(&body).ok();
        let message = value
            .as_ref()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
            .unwrap_or_else(|| if body.is_empty() { status.to_string() } else { body.clone() });
        let code = value.as_ref().and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned));
        return Err(CliHttp { status: Some(status.as_u16()), code, message, timed_out: false, connect: false });
    }
    serde_json::from_str(&body).map_err(|e| CliHttp {
        status: Some(status.as_u16()), code: None, message: format!("invalid coordinator response: {e}"), timed_out: false, connect: false,
    })
}

pub fn get<T: for<'de> Deserialize<'de>>(session: &Session, url: &str) -> Result<T, CliHttp> {
    let response = session
        .request(reqwest::Method::GET, url)
        .map_err(|message| CliHttp { status: None, code: None, message, timed_out: false, connect: false })?
        .timeout(METADATA_TIMEOUT)
        .send()
        .map_err(CliHttp::transport)?;
    decode(response)
}

pub fn fetch_cluster(session: &Session, server: &str) -> Result<Cluster, CliHttp> {
    get(session, &endpoint(server, "/v1/cluster"))
}

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

pub fn find_query_by_tag(session: &Session, server: &str, tag: &str) -> Result<Option<QueryRecord>, CliHttp> {
    let records: Vec<QueryRecord> = get(session, &endpoint(server, "/v1/query"))?;
    Ok(records.into_iter().find(|r| r.context.client_tags.iter().any(|t| t == tag)))
}

pub fn cancel_query(session: &Session, server: &str, id: &str) -> Result<(), CliHttp> {
    let url = url_with_segments(server, "/v1/query", &[id])?;
    let response = session
        .request(reqwest::Method::DELETE, &url)
        .map_err(|message| CliHttp { status: None, code: None, message, timed_out: false, connect: false })?
        .timeout(METADATA_TIMEOUT)
        .send()
        .map_err(CliHttp::transport)?;
    if response.status().is_success() || response.status().as_u16() == 409 {
        return Ok(());
    }
    decode::<serde_json::Value>(response).map(|_| ())
}
```

- [ ] **Step 6: Wire modules in `main.rs`** — add `mod client;` and `mod theme;` at the top (nothing calls them yet; allow dead code on the modules for this commit with `#[allow(dead_code)]` on the `mod` lines, removed in Task 5).

- [ ] **Step 7: Run tests**

Run: `cargo test -p kaveon-cli`
Expected: PASS (new + existing).

- [ ] **Step 8: Commit**

```bash
git add crates/cli/src/theme.rs crates/cli/src/client crates/cli/src/main.rs
git commit -m "cli: typed cluster, whoami and query-record client with a theme"
```

### Task 4: `render/cluster.rs` — the header

**Files:**
- Create: `engine/crates/cli/src/render/mod.rs`, `engine/crates/cli/src/render/cluster.rs`
- Modify: `engine/crates/cli/src/main.rs` (`mod render;`)

**Interfaces:**
- Produces: `render::cluster::header(cli_version: &str, server: &str, cluster: Option<&Cluster>, whoami: Option<&Whoami>, auth_mode: &str, insecure_development: bool, user: &str, now_unix: u64, theme: &Theme) -> Vec<ratatui::text::Line<'static>>` and `render::to_plain(lines: &[Line]) -> String` (ANSI-free text with `\n`).

- [ ] **Step 1: Failing test** in `render/cluster.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session::{Admission, Cluster, Node, Whoami};

    fn node(id: &str, hb: u64) -> Node {
        Node { node_id: id.into(), address: String::new(), version: "0.1.0".into(), environment: "docker".into(), last_heartbeat: hb, admission: None }
    }

    #[test]
    fn header_shows_engine_cluster_and_session_lines() {
        let mut coordinator = node("coordinator-1", 1000);
        coordinator.admission = Some(Admission { limit_bytes: 4 * 1024 * 1024 * 1024, queue_depth: 0 });
        let cluster = Cluster { environment: "docker".into(), coordinator, workers: vec![node("w1", 1000), node("w2", 900)] };
        let whoami = Whoami { principal: "prproddu".into(), display: None, role: "admin".into(), auth: "development".into() };
        let lines = header("0.3.0", "http://localhost:8081", Some(&cluster), Some(&whoami), "none", true, "prproddu", 1000, &crate::theme::Theme::mono());
        let text = crate::render::to_plain(&lines);
        assert!(text.starts_with("  KAVEON  v0.3.0\n"));
        assert!(text.contains("Engine    http://localhost:8081  ·  v0.1.0  ·  docker"));
        assert!(text.contains("Cluster   coordinator-1  ·  1 worker ready  ·  1 stale  ·  4.0 GiB admission"));
        assert!(text.contains("Session   prproddu  ·  admin  ·  auth none (insecure development)"));
        assert!(text.contains("SQL ends with ;   .help for commands   Ctrl-C cancels a running query"));
    }

    #[test]
    fn header_without_cluster_or_whoami_degrades() {
        let lines = header("0.3.0", "https://engine.example", None, None, "auto", false, "ana", 0, &crate::theme::Theme::mono());
        let text = crate::render::to_plain(&lines);
        assert!(text.contains("Cluster   unavailable"));
        assert!(text.contains("Session   ana  ·  auth auto"));
        assert!(!text.contains("insecure"));
    }

    #[test]
    fn zero_workers_is_called_out() {
        let cluster = Cluster { environment: "docker".into(), coordinator: node("c", 1), workers: vec![] };
        let text = crate::render::to_plain(&header("0.3.0", "http://localhost:8081", Some(&cluster), None, "none", true, "x", 1, &crate::theme::Theme::mono()));
        assert!(text.contains("no workers — statements run on the coordinator"));
    }
}
```

- [ ] **Step 2: Run** — `cargo test -p kaveon-cli render::cluster` → FAIL to compile.

- [ ] **Step 3: Implement `render/mod.rs`**

```rust
pub mod cluster;

use ratatui::text::Line;

/// The lines as text, no styling: what non-TTY output and tests see.
pub fn to_plain(lines: &[Line<'_>]) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            out.push_str(&span.content);
        }
        out.push('\n');
    }
    out
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

pub fn human_count(count: u64) -> String {
    match count {
        0..=9_999 => count.to_string(),
        10_000..=999_999 => format!("{:.1}K", count as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", count as f64 / 1e6),
        _ => format!("{:.2}B", count as f64 / 1e9),
    }
}

pub fn thousands(value: i128) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if negative { format!("-{out}") } else { out }
}
```

- [ ] **Step 4: Implement `render/cluster.rs`**

```rust
use crate::client::session::{Cluster, Whoami};
use crate::render::human_bytes;
use crate::theme::Theme;
use ratatui::text::{Line, Span};

const RULE: &str = "─────────────────────────────────────────────────────────────";

fn row(label: &str, parts: Vec<Span<'static>>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("  {label:<9} "), theme.dim)];
    spans.extend(parts);
    Line::from(spans)
}

fn joined(parts: &[String]) -> String {
    parts.join("  ·  ")
}

#[allow(clippy::too_many_arguments)]
pub fn header(
    cli_version: &str,
    server: &str,
    cluster: Option<&Cluster>,
    whoami: Option<&Whoami>,
    auth_mode: &str,
    insecure_development: bool,
    user: &str,
    now_unix: u64,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("  KAVEON", theme.title),
            Span::styled(format!("  v{cli_version}"), theme.dim),
        ]),
        Line::from(Span::styled(format!("  {RULE}"), theme.dim)),
    ];
    match cluster {
        Some(cluster) => {
            lines.push(row(
                "Engine",
                vec![Span::raw(joined(&[
                    server.to_owned(),
                    format!("v{}", cluster.coordinator.version),
                    cluster.environment.clone(),
                ]))],
                theme,
            ));
            let (ready, stale) = cluster.ready_workers(now_unix);
            let mut parts = vec![cluster.coordinator.node_id.clone()];
            let mut style = ratatui::style::Style::default();
            if cluster.workers.is_empty() {
                parts.push("no workers — statements run on the coordinator".into());
                style = theme.warning;
            } else {
                parts.push(format!("{ready} worker{} ready", if ready == 1 { "" } else { "s" }));
                if stale > 0 {
                    parts.push(format!("{stale} stale"));
                    style = theme.warning;
                }
            }
            if let Some(limit) = cluster.admission_limit_bytes() {
                parts.push(format!("{} admission", human_bytes(limit)));
            }
            lines.push(row("Cluster", vec![Span::styled(joined(&parts), style)], theme));
        }
        None => {
            lines.push(row("Engine", vec![Span::raw(server.to_owned())], theme));
            lines.push(row("Cluster", vec![Span::styled("unavailable", theme.warning)], theme));
        }
    }
    let mut session = Vec::new();
    match whoami {
        Some(who) => {
            session.push(who.display.clone().unwrap_or_else(|| who.principal.clone()));
            if !who.role.is_empty() {
                session.push(who.role.clone());
            }
        }
        None => session.push(user.to_owned()),
    }
    session.push(if insecure_development {
        format!("auth {auth_mode} (insecure development)")
    } else {
        format!("auth {auth_mode}")
    });
    lines.push(row("Session", vec![Span::raw(joined(&session))], theme));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  SQL ends with ;   .help for commands   Ctrl-C cancels a running query",
        theme.dim,
    )));
    lines.push(Line::raw(""));
    lines
}
```

`insecure_development` is true when `whoami.auth == "development"` or, without whoami, when `auth_mode == "none"` and the server is loopback (caller computes it).

- [ ] **Step 5: Run tests** — `cargo test -p kaveon-cli render` → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/render crates/cli/src/main.rs
git commit -m "cli: the session header from the cluster and whoami payloads"
```

### Task 5: The inline shell — editor, status bar, header, statements through the existing path

This is the intro the architect tests. After this task, a TTY session shows the header, a pinned editor, the status bar, and runs statements (still through the existing `remote::execute`, printed plainly into scrollback). Progress/cancel come in Stage 2.

**Files:**
- Create: `engine/crates/cli/src/shell/mod.rs`, `shell/app.rs`, `shell/editor.rs`, `shell/status.rs`
- Modify: `engine/crates/cli/src/remote.rs` — make `execute`, `handle_meta_command`, `print_remote_help` `pub(crate)`; extract `pub(crate) fn execute_to_string(client, options, sql) -> Result<String, String>` from `execute` (everything `execute` does, returning the output instead of calling `write_output`); `run` dispatches to `shell::run` when stdin and stdout are TTYs.
- Modify: `engine/crates/cli/src/args.rs` — add `theme: String` (`--theme`, default `"dark"`), `no_header: bool` (`--no-header`), `width: Option<u16>` (`--width`), keep `--disable-auto-suggestion` as a no-op flag.

**Interfaces:**
- Produces: `shell::run(session: &Session, options: &mut Options) -> Result<(), String>`; `shell::editor::Editor { new(), lines() -> String, is_complete() -> bool, clear(), set_history(Vec<String>), push_history(String), handle(&KeyEvent) -> EditorAction }`, `EditorAction { None, Submit(String), Quit, Interrupt, Clear }`; `shell::status::{box_title, status_line}`.

- [ ] **Step 1: Args** — in `Options` add `pub theme: String, pub no_header: bool, pub width: Option<u16>`; defaults `"dark"`, `false`, `None`; parse `--theme` (accept `dark|light|mono`, else error `--theme expects dark, light, or mono`), `--no-header` (flag; add to both flag lists in `normalize_args` and `parse_with_config`), `--width` (`u16 > 0`). Add `"theme"` and `"width"` to the config-file allowlist. Add to `print_usage`:

```
      --theme <NAME>          dark (default), light, or mono
      --no-header             Skip the session header
      --width <COLS>          Terminal width for result tables (default: detected)
```

Test (append to `args.rs` tests):

```rust
    #[test]
    fn parses_shell_presentation_flags() {
        let Command::Run(options) = parse(&strings(&["kaveon", "--theme", "mono", "--no-header", "--width", "100"])).unwrap() else { panic!() };
        assert_eq!(options.theme, "mono");
        assert!(options.no_header);
        assert_eq!(options.width, Some(100));
        assert!(parse(&strings(&["kaveon", "--theme", "neon"])).is_err());
    }
```

Run `cargo test -p kaveon-cli args` → PASS after implementing.

- [ ] **Step 2: Editor tests** in `shell/editor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    fn type_text(editor: &mut Editor, text: &str) {
        for ch in text.chars() {
            editor.handle(&key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn enter_submits_only_a_terminated_statement() {
        let mut editor = Editor::new("EMACS");
        type_text(&mut editor, "SELECT 1");
        assert!(matches!(editor.handle(&key(KeyCode::Enter)), EditorAction::None));
        assert_eq!(editor.lines(), "SELECT 1\n");
        type_text(&mut editor, "FROM t;");
        assert!(matches!(editor.handle(&key(KeyCode::Enter)), EditorAction::Submit(sql) if sql == "SELECT 1\nFROM t;"));
        assert_eq!(editor.lines(), "");
    }

    #[test]
    fn trailing_comment_after_semicolon_still_submits() {
        let mut editor = Editor::new("EMACS");
        type_text(&mut editor, "SELECT 1; -- done");
        assert!(matches!(editor.handle(&key(KeyCode::Enter)), EditorAction::Submit(_)));
    }

    #[test]
    fn dot_commands_submit_without_semicolon() {
        let mut editor = Editor::new("EMACS");
        type_text(&mut editor, ".tables");
        assert!(matches!(editor.handle(&key(KeyCode::Enter)), EditorAction::Submit(sql) if sql == ".tables"));
    }

    #[test]
    fn ctrl_enter_forces_submit_and_history_navigates() {
        let mut editor = Editor::new("EMACS");
        editor.set_history(vec!["SELECT 1;".into(), "SELECT 2;".into()]);
        editor.handle(&key(KeyCode::Up));
        assert_eq!(editor.lines(), "SELECT 2;");
        editor.handle(&key(KeyCode::Up));
        assert_eq!(editor.lines(), "SELECT 1;");
        editor.handle(&key(KeyCode::Down));
        assert_eq!(editor.lines(), "SELECT 2;");
        editor.clear();
        type_text(&mut editor, "SELECT 3");
        assert!(matches!(editor.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)), EditorAction::Submit(sql) if sql == "SELECT 3"));
    }

    #[test]
    fn ctrl_c_and_ctrl_d_map_to_actions() {
        let mut editor = Editor::new("EMACS");
        assert!(matches!(editor.handle(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), EditorAction::Interrupt));
        assert!(matches!(editor.handle(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)), EditorAction::Quit));
        type_text(&mut editor, "x");
        assert!(matches!(editor.handle(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)), EditorAction::None));
    }
}
```

- [ ] **Step 3: Run** — `cargo test -p kaveon-cli shell::editor` → FAIL to compile.

- [ ] **Step 4: Implement `shell/editor.rs`**

```rust
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::widgets::Block;
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};
use tui_textarea::{CursorMove, Input, Key, TextArea};

pub enum EditorAction {
    None,
    Submit(String),
    Quit,
    Interrupt,
    Clear,
}

pub struct Editor {
    area: TextArea<'static>,
    vi: bool,
    history: Vec<String>,
    /// Index into `history` while browsing; `None` when editing a new statement.
    browsing: Option<usize>,
    draft: String,
}

impl Editor {
    pub fn new(editing_mode: &str) -> Editor {
        let mut area = TextArea::default();
        area.set_cursor_line_style(Style::default());
        area.set_tab_length(4);
        Editor { area, vi: editing_mode.eq_ignore_ascii_case("VI"), history: Vec::new(), browsing: None, draft: String::new() }
    }

    pub fn lines(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.area.lines().iter().all(|line| line.trim().is_empty())
    }

    pub fn clear(&mut self) {
        self.area = TextArea::default();
        self.area.set_cursor_line_style(Style::default());
        self.area.set_tab_length(4);
        self.browsing = None;
    }

    pub fn set_text(&mut self, text: &str) {
        self.clear();
        self.area.insert_str(text);
        self.area.move_cursor(CursorMove::End);
    }

    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
    }

    pub fn push_history(&mut self, statement: String) {
        if self.history.last() != Some(&statement) {
            self.history.push(statement);
        }
        self.browsing = None;
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// A statement is complete when, ignoring trailing whitespace and
    /// comments, it ends with `;` — or it is a dot command / bare alias.
    pub fn is_complete(&self) -> bool {
        let text = self.lines();
        let trimmed = text.trim();
        if trimmed.starts_with('.') || is_bare_alias(trimmed) {
            return !trimmed.contains('\n');
        }
        let tokens = Tokenizer::new(&GenericDialect {}, trimmed).tokenize().unwrap_or_default();
        tokens.iter().rev().find(|t| !matches!(t, Token::Whitespace(_))).is_some_and(|t| matches!(t, Token::SemiColon))
    }

    pub fn handle(&mut self, key: &KeyEvent) -> EditorAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return EditorAction::Interrupt,
            (KeyCode::Char('d'), true) if self.is_empty() => return EditorAction::Quit,
            (KeyCode::Char('d'), true) => return EditorAction::None,
            (KeyCode::Char('l'), true) => return EditorAction::Clear,
            (KeyCode::Enter, true) => return self.submit(),
            (KeyCode::Enter, false) if key.modifiers.contains(KeyModifiers::ALT) => return self.submit(),
            (KeyCode::Enter, false) => {
                if self.is_complete() {
                    return self.submit();
                }
                self.area.insert_newline();
                return EditorAction::None;
            }
            (KeyCode::Up, false) if self.area.cursor().0 == 0 => return self.history_back(),
            (KeyCode::Down, false) if self.area.cursor().0 + 1 == self.area.lines().len() => return self.history_forward(),
            _ => {}
        }
        let input: Input = (*key).into();
        if let Input { key: Key::Null, .. } = input {
            return EditorAction::None;
        }
        self.area.input(input);
        self.browsing = None;
        EditorAction::None
    }

    fn submit(&mut self) -> EditorAction {
        let text = self.lines().trim().to_owned();
        if text.is_empty() {
            return EditorAction::None;
        }
        self.clear();
        EditorAction::Submit(text)
    }

    fn history_back(&mut self) -> EditorAction {
        if self.history.is_empty() {
            return EditorAction::None;
        }
        let next = match self.browsing {
            None => {
                self.draft = self.lines();
                self.history.len() - 1
            }
            Some(0) => return EditorAction::None,
            Some(i) => i - 1,
        };
        let text = self.history[next].clone();
        self.set_text(&text);
        self.browsing = Some(next);
        EditorAction::None
    }

    fn history_forward(&mut self) -> EditorAction {
        let Some(current) = self.browsing else { return EditorAction::None };
        if current + 1 < self.history.len() {
            let text = self.history[current + 1].clone();
            self.set_text(&text);
            self.browsing = Some(current + 1);
        } else {
            let draft = self.draft.clone();
            self.set_text(&draft);
            self.browsing = None;
        }
        EditorAction::None
    }

    pub fn widget(&mut self, title: &str, theme: &Theme, running: bool) -> &TextArea<'static> {
        let border = if running { theme.dim } else { theme.accent };
        self.area.set_block(
            Block::bordered()
                .border_style(border)
                .title(ratatui::text::Span::styled(format!(" {title} "), theme.title)),
        );
        self.area.set_style(if running { theme.dim } else { Style::default() });
        &self.area
    }

    pub fn height(&self, max: u16) -> u16 {
        let lines = self.area.lines().len() as u16;
        (lines + 2).clamp(3, max.max(3))
    }

    pub fn vi(&self) -> bool {
        self.vi
    }
}

fn is_bare_alias(text: &str) -> bool {
    let word = text.trim_end_matches(';').trim();
    ["exit", "quit", "help", "clear"].iter().any(|alias| word.eq_ignore_ascii_case(alias))
}
```

SQL highlighting is added in Task 12 (Stage 5) once the shell exists; the widget here uses the theme's default text style. `vi` mode maps keys through `tui-textarea`'s vi example in Task 12 as well; until then `--editing-mode vi` is accepted and edits in insert mode.

- [ ] **Step 5: `shell/status.rs`**

```rust
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
        Some((ready, 0)) => parts.push(format!("{ready} worker{}", if ready == 1 { "" } else { "s" })),
        Some((ready, stale)) => {
            parts.push(format!("{ready} workers · {stale} stale"));
            style = theme.warning;
        }
        None => {}
    }
    if let Some(ms) = facts.last_elapsed_ms {
        parts.push(format!("last {:.2} s", ms as f64 / 1000.0));
    }
    if let Some(rows) = facts.last_scanned_rows {
        parts.push(format!("{} rows scanned", crate::render::human_count(rows)));
    }
    Line::from(Span::styled(format!(" {}", parts.join(" · ")), style))
}

pub fn host_of(server: &str) -> String {
    reqwest::Url::parse(server)
        .ok()
        .and_then(|url| url.host_str().map(|h| match url.port() { Some(p) => format!("{h}:{p}"), None => h.to_owned() }))
        .unwrap_or_else(|| server.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_lists_host_workers_and_last_query() {
        let facts = StatusFacts { host: "localhost:8081", workers_ready: Some((2, 0)), last_elapsed_ms: Some(1100), last_scanned_rows: Some(18_000_000) };
        let line = status_line(&facts, &Theme::mono());
        assert_eq!(crate::render::to_plain(&[line]), " localhost:8081 · 2 workers · last 1.10 s · 18.0M rows scanned\n");
        assert_eq!(host_of("http://localhost:8081/"), "localhost:8081");
    }
}
```

- [ ] **Step 6: `shell/app.rs` and `shell/mod.rs`** — the event loop. Statement execution goes through `remote::execute_to_string` on the UI thread for this task (blocking; Stage 2 moves it to the worker thread).

`shell/mod.rs`:

```rust
pub mod app;
pub mod editor;
pub mod status;

pub use app::run;
```

`shell/app.rs`:

```rust
use crate::args::Options;
use crate::auth::Session;
use crate::client::session::{self as api, Cluster, Whoami};
use crate::render;
use crate::shell::editor::{Editor, EditorAction};
use crate::shell::status::{StatusFacts, box_title, host_of, status_line};
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VIEWPORT_MAX: u16 = 16;

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct App {
    editor: Editor,
    theme: Theme,
    cluster: Option<Cluster>,
    whoami: Option<Whoami>,
    last_elapsed_ms: Option<u64>,
    last_scanned_rows: Option<u64>,
    history_path: Option<std::path::PathBuf>,
}

impl App {
    fn status_facts<'a>(&'a self, host: &'a str) -> StatusFacts<'a> {
        StatusFacts {
            host,
            workers_ready: self.cluster.as_ref().map(|c| c.ready_workers(now_unix())),
            last_elapsed_ms: self.last_elapsed_ms,
            last_scanned_rows: self.last_scanned_rows,
        }
    }
}

pub fn run(session: &Session, options: &mut Options) -> Result<(), String> {
    let theme = Theme::detect(&options.theme, true);
    let cluster = api::fetch_cluster(session, &options.server).ok();
    let whoami = api::fetch_whoami(session, &options.server).ok().flatten();
    let insecure = whoami.as_ref().is_some_and(|w| w.auth == "development")
        || (options.auth == "none" && whoami.is_none());
    let history_path = (!options.no_history)
        .then(|| options.history_file.clone().or_else(crate::input::default_history_file))
        .flatten();
    let mut app = App {
        editor: Editor::new(&options.editing_mode),
        theme,
        cluster,
        whoami,
        last_elapsed_ms: None,
        last_scanned_rows: None,
        history_path,
    };
    if let Some(path) = &app.history_path
        && let Ok(text) = std::fs::read_to_string(path)
    {
        app.editor.set_history(text.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect());
    }

    enable_raw_mode().map_err(|e| format!("cannot enter raw mode: {e}"))?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(VIEWPORT_MAX) })
        .map_err(|e| format!("cannot initialize terminal: {e}"))?;
    let result = event_loop(&mut terminal, &mut app, session, options);
    let _ = disable_raw_mode();
    let _ = terminal.clear_after_cursor();
    println!();
    save_history(&app);
    result
}

fn save_history(app: &App) {
    if let Some(path) = &app.history_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tail: Vec<&str> = app.editor.history().iter().rev().take(1000).rev().map(String::as_str).collect();
        let _ = std::fs::write(path, tail.join("\n") + "\n");
    }
}

/// Push finished lines above the viewport (normal scrollback).
fn emit(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, lines: Vec<Line<'static>>) -> Result<(), String> {
    let height = lines.len() as u16;
    if height == 0 {
        return Ok(());
    }
    terminal
        .insert_before(height, |buf| {
            Paragraph::new(Text::from(lines)).render(buf.area, buf);
        })
        .map_err(|e| e.to_string())
}

use ratatui::widgets::Widget;

fn emit_text(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, text: &str, theme: &Theme, dim: bool) -> Result<(), String> {
    let lines = text
        .lines()
        .map(|l| if dim { Line::styled(l.to_owned(), theme.dim) } else { Line::raw(l.to_owned()) })
        .collect();
    emit(terminal, lines)
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &Session,
    options: &mut Options,
) -> Result<(), String> {
    let host = host_of(&options.server);
    if !options.no_header {
        let header = render::cluster::header(
            env!("CARGO_PKG_VERSION"),
            &options.server,
            app.cluster.as_ref(),
            app.whoami.as_ref(),
            &options.auth,
            app.whoami.as_ref().is_some_and(|w| w.auth == "development") || (options.auth == "none" && app.whoami.is_none()),
            &options.user,
            now_unix(),
            &app.theme,
        );
        emit(terminal, header)?;
    }
    let mut last_cluster_poll = std::time::Instant::now();
    loop {
        let title = box_title(&options.catalog, &options.schema);
        terminal
            .draw(|frame| {
                let editor_height = app.editor.height(VIEWPORT_MAX - 1);
                let [editor_area, status_area] =
                    Layout::vertical([Constraint::Length(editor_height), Constraint::Length(1)]).areas(frame.area());
                frame.render_widget(app.editor.widget(&title, &app.theme, false), editor_area);
                frame.render_widget(Paragraph::new(status_line(&app.status_facts(&host), &app.theme)), status_area);
            })
            .map_err(|e| e.to_string())?;

        if !event::poll(Duration::from_millis(66)).map_err(|e| e.to_string())? {
            if last_cluster_poll.elapsed() >= Duration::from_secs(30) {
                if let Ok(cluster) = api::fetch_cluster(session, &options.server) {
                    app.cluster = Some(cluster);
                }
                last_cluster_poll = std::time::Instant::now();
            }
            continue;
        }
        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.editor.handle(&key) {
            EditorAction::None => {}
            EditorAction::Quit => return Ok(()),
            EditorAction::Clear => {
                terminal.clear().map_err(|e| e.to_string())?;
            }
            EditorAction::Interrupt => {
                if app.editor.is_empty() {
                    emit_text(terminal, "Ctrl-D or .quit to exit", &app.theme, true)?;
                } else {
                    app.editor.clear();
                }
            }
            EditorAction::Submit(text) => {
                app.editor.push_history(text.clone());
                let echo = Line::styled(format!("{title}> {}", text.replace('\n', "\n   ")), app.theme.dim);
                emit(terminal, vec![echo])?;
                let trimmed = text.trim_end_matches(';').trim();
                if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") || text == ".quit" || text == ".exit" || text == ".q" {
                    return Ok(());
                }
                let output = run_statement(session, options, &text);
                match output {
                    Ok(out) => emit_text(terminal, &out, &app.theme, false)?,
                    Err(err) => emit(terminal, vec![Line::styled(format!("error: {err}"), app.theme.error)])?,
                }
                emit(terminal, vec![Line::raw("")])?;
            }
        }
    }
}

/// Stage 1: statements run through the existing path and return their
/// plain output. Stage 2 replaces this with the worker thread and progress.
fn run_statement(session: &Session, options: &mut Options, text: &str) -> Result<String, String> {
    if text.starts_with('.') {
        return crate::remote::meta_command_to_string(session, options, text);
    }
    let mut out = String::new();
    for statement in crate::input::split_statements(text)? {
        out.push_str(&crate::remote::execute_to_string(session, options, &statement)?);
    }
    Ok(out)
}
```

In `remote.rs`:
- `pub(crate) fn execute_to_string(client, options, sql) -> Result<String, String>`: the body of `execute` up to and including building `output` (including the telemetry footer), returning `output` instead of `write_output`. `execute` becomes `write_output(options, &execute_to_string(...)?)`.
- `pub(crate) fn meta_command_to_string(client, options, command) -> Result<String, String>`: like `handle_meta_command` but `.help` returns the help text as a `String` (make `print_remote_help` return `String`, and `remote_help_text()` used by both), `.quit` returns `Ok(String::new())` (the shell handles quit itself), metadata commands capture `print_metadata`'s output — change `print_metadata` and the `Describe`/`Use` arms of `run_meta_command` to return `String` (`run_meta_command -> Result<String, String>`), and the old callers print it.
- `run`: replace the `repl(&client, options)` call with:

```rust
    if io::stdout().is_terminal() {
        return crate::shell::run(&client, options);
    }
    repl(&client, options)
```

(`repl` stays until Task 6 deletes it.)

- Move `default_history_file` in `input.rs` to `pub fn`.

- [ ] **Step 7: Build and test** — `cargo test -p kaveon-cli` → PASS; `cargo clippy -p kaveon-cli --all-targets -- -D warnings` clean.

- [ ] **Step 8: Manual check against the local stack** (`docs/guides/local-stack.md` running):

```powershell
cargo build --release -p kaveon-cli
.\target\release\kaveon.exe http://localhost:8081/OpenSource/kaveon_product --auth none
```

Expect: header with `2 workers ready`, `Session   local-admin …` or the `--user` fallback, the bordered editor titled `OpenSource.kaveon_product`, the status line; `SHOW TABLES;` prints the table into scrollback; `SELECT COUNT(*) FROM kaveon_events_users;` prints result + footer; Up recalls; Ctrl-D exits and `%APPDATA%\kaveon\history` has the statements. Piped: `echo "SELECT 1;" | kaveon … ` still prints plain output and no header.

- [ ] **Step 9: Commit**

```bash
git add crates/cli/src
git commit -m "cli: inline shell — session header, pinned editor and status bar"
```

**Checkpoint: tell the architect the intro is testable.**

### Task 6: Remove rustyline; batch module

**Files:**
- Create: `engine/crates/cli/src/batch.rs` (move `execute_script` and the non-TTY branches of `remote::run`)
- Modify: `engine/crates/cli/src/input.rs` (delete `TerminalInput`, `SqlHelper`, `COMPLETIONS`, the rustyline test; keep `default_history_file`, `read_file`, splitters and their tests)
- Modify: `engine/crates/cli/src/remote.rs` (delete `repl`), `main.rs`, `Cargo.toml` (drop `rustyline`, `terminal_size` — `terminal_size` is replaced by `crossterm::terminal::size` in `output.rs::auto`)

- [ ] **Step 1: `batch.rs`**

```rust
//! Non-interactive execution: `-e`, `-f`, and piped standard input.
use crate::args::Options;
use crate::auth::Session;
use crate::input;
use std::io::{self, Read};

pub fn run(session: &Session, options: &mut Options) -> Result<(), String> {
    if let Some(sql) = options.execute.clone() {
        return execute_script(session, options, &sql, options.ignore_errors);
    }
    if let Some(path) = options.file.clone() {
        let script = input::read_file(&path)?;
        return execute_script(session, options, &script, options.ignore_errors);
    }
    let mut script = String::new();
    io::stdin().read_to_string(&mut script).map_err(|e| format!("cannot read standard input: {e}"))?;
    execute_script(session, options, &script, options.ignore_errors)
}

pub fn execute_script(session: &Session, options: &mut Options, script: &str, ignore_errors: bool) -> Result<(), String> {
    let mut first_error = None;
    for statement in input::split_statements(script)? {
        if let Err(error) = crate::remote::execute(session, options, &statement) {
            if !ignore_errors {
                return Err(error);
            }
            eprintln!("error: {error}");
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}
```

`remote::run` becomes:

```rust
pub fn run(options: &mut Options) -> Result<(), String> {
    let client = Session::connect(options)?;
    let interactive = options.execute.is_none() && options.file.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive {
        return crate::batch::run(&client, options);
    }
    if let Some(format) = options.output_format_interactive {
        options.output_format = format;
    }
    crate::shell::run(&client, options)
}
```

- [ ] **Step 2: `output.rs::auto`** — replace the `terminal_size` call with `crossterm::terminal::size().ok().map(|(w, _)| usize::from(w))`.

- [ ] **Step 3: Delete rustyline code and the two dependencies; `cargo test -p kaveon-cli`, clippy clean.**

- [ ] **Step 4: Run the packaged-binary contract** — `cargo build --release -p kaveon-cli && python ../engine/qualification/cli_workflows.py --cli target/release/kaveon.exe` → passes unchanged (it uses `-e`, `--file` and piped stdin).

- [ ] **Step 5: Commit** — `git commit -am "cli: batch module; rustyline retired"`.

---

## Stage 2 — Statement thread, live running line, cancel

### Task 7: `client/statement.rs` — submit on a worker thread with a tag

**Files:**
- Create: `engine/crates/cli/src/client/statement.rs`
- Modify: `client/mod.rs`

**Interfaces:**
- Produces:
  - `StatementRequest { sql: String, catalog, schema, user, source, client_tags: Vec<String>, result_delivery: &'static str, settings: Option<serde_json::Map<String, Value>> }`
  - `StatementResult { id: String, state: String, columns: Vec<Column>, rows: Vec<Vec<Value>>, elapsed_ms: u64, next_uri: Option<String> }`, `Column { name, data_type }`
  - `Handle { tag: String, events: mpsc::Receiver<StatementEvent>, started: Instant }`
  - `enum StatementEvent { Finished(StatementResult), Failed(CliHttp) }`
  - `fn submit(session: Arc<Session>, server: String, request: StatementRequest, timeout: Duration) -> Handle` — spawns the thread; the tag `kaveon-cli:<uuid v4>` is appended to `client_tags` and returned in the handle.

`auth::Session` holds a `RefCell` and is not `Sync`; wrap the session in `Arc<Mutex<Session>>` inside `client/statement.rs` (`pub type SharedSession = Arc<std::sync::Mutex<Session>>`) and lock only for the duration of building + sending the request. The UI thread's polls lock the same mutex briefly; the worker thread's `send()` holds it — so the worker must build the request under the lock, then **release the lock before `send()`**: `request()` returns a `RequestBuilder` that owns its client clone; `send()` runs outside the lock.

- [ ] **Step 1: Failing test** (fixture from Task 3, moved to `client/test_server.rs` as `pub(crate) fn fixture` with `#[cfg(test)]`):

```rust
    #[test]
    fn submit_tags_the_request_and_reports_the_result() {
        let (url, thread) = fixture(vec![("POST /v1/statement ", 200, r#"{"id":"q1","state":"FINISHED","columns":[{"name":"n","type":"Int64"}],"data":[[1]],"elapsed_ms":3}"#.into())]);
        let (session, options) = session(&url);
        let shared = Arc::new(Mutex::new(session));
        let handle = submit(shared, options.server.clone(), StatementRequest::new("SELECT 1", &options), Duration::from_secs(5));
        assert!(handle.tag.starts_with("kaveon-cli:"));
        match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            StatementEvent::Finished(result) => {
                assert_eq!(result.id, "q1");
                assert_eq!(result.rows, vec![vec![serde_json::json!(1)]]);
            }
            StatementEvent::Failed(f) => panic!("{f:?}"),
        }
        thread.join().unwrap();
    }
```

Extend the fixture to capture request bodies (return them from the join handle as `Vec<String>`) and assert the body's `client_tags` contains the tag and `result_delivery == "paged"`.

- [ ] **Step 2: Implement**

```rust
use crate::args::Options;
use crate::auth::Session;
use crate::client::session::CliHttp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub type SharedSession = Arc<Mutex<Session>>;

#[derive(Serialize, Clone)]
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

pub enum StatementEvent {
    Finished(StatementResult),
    Failed(CliHttp),
}

pub struct Handle {
    pub tag: String,
    pub events: mpsc::Receiver<StatementEvent>,
    pub started: Instant,
}

pub fn new_tag() -> String {
    format!("kaveon-cli:{}", uuid::Uuid::new_v4())
}

pub fn submit(session: SharedSession, server: String, mut request: StatementRequest, timeout: Duration) -> Handle {
    let tag = new_tag();
    request.client_tags.push(tag.clone());
    let (sender, events) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let url = format!("{}/v1/statement", server.trim_end_matches('/'));
        let builder = {
            let guard = match session.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    let _ = sender.send(StatementEvent::Failed(CliHttp { status: None, code: None, message: "session lock poisoned".into(), timed_out: false, connect: false }));
                    return;
                }
            };
            guard.request(reqwest::Method::POST, &url)
        };
        let builder = match builder {
            Ok(builder) => builder,
            Err(message) => {
                let _ = sender.send(StatementEvent::Failed(CliHttp { status: None, code: None, message, timed_out: false, connect: false }));
                return;
            }
        };
        let event = match builder.timeout(timeout).json(&request).send() {
            Err(error) => StatementEvent::Failed(CliHttp { status: None, code: None, message: error.to_string(), timed_out: error.is_timeout(), connect: error.is_connect() }),
            Ok(response) => match crate::client::session::decode::<StatementResult>(response) {
                Ok(result) if result.error.is_some() => StatementEvent::Failed(CliHttp { status: Some(200), code: None, message: result.error.clone().unwrap_or_default(), timed_out: false, connect: false }),
                Ok(result) => StatementEvent::Finished(result),
                Err(failure) => StatementEvent::Failed(failure),
            },
        };
        let _ = sender.send(event);
    });
    Handle { tag, events, started }
}
```

Make `client::session::decode` `pub(crate)`.

- [ ] **Step 3: Tests pass; commit** — `git commit -m "cli: statements run on a worker thread, tagged for discovery"`.

### Task 8: `shell/progress.rs` and the running loop with Ctrl-C cancel

**Files:**
- Create: `engine/crates/cli/src/shell/progress.rs`
- Modify: `engine/crates/cli/src/shell/app.rs`

**Interfaces:**
- Produces: `Progress { phase: Phase, elapsed: Duration, query_id: Option<String>, admission_wait_ms: u64, queue_ahead: Option<u64>, tasks_done: usize, tasks_total: usize, workers: usize, rows_scanned: u64, cancelling: bool }`, `enum Phase { Submitting, Queued, Running, Cancelling }`, `Progress::from_record(record: &QueryRecord, elapsed) -> Progress`, `fn line(progress: &Progress, tick: usize, theme: &Theme) -> Line<'static>`.

- [ ] **Step 1: Tests**

```rust
    #[test]
    fn running_line_reports_state_and_counters_only_when_known() {
        let theme = Theme::mono();
        let submitting = Progress { phase: Phase::Submitting, elapsed: Duration::from_millis(400), ..Progress::default() };
        assert_eq!(crate::render::to_plain(&[line(&submitting, 0, &theme)]), " ⠋ Submitting 0.4 s                                                     Ctrl-C to cancel\n".trim_end().to_owned() + "\n");
        let queued = Progress { phase: Phase::Queued, elapsed: Duration::from_millis(3200), queue_ahead: Some(2), ..Progress::default() };
        assert!(crate::render::to_plain(&[line(&queued, 1, &theme)]).contains("Queued 3.2 s for memory admission · 2 ahead"));
        let running = Progress { phase: Phase::Running, elapsed: Duration::from_millis(2400), tasks_done: 3, tasks_total: 5, workers: 2, rows_scanned: 210_000_000, ..Progress::default() };
        let text = crate::render::to_plain(&[line(&running, 2, &theme)]);
        assert!(text.contains("Running 2.4 s · 3/5 tasks · 2 workers · 210.0M rows scanned"));
        let bare = Progress { phase: Phase::Running, elapsed: Duration::from_secs(1), workers: 2, ..Progress::default() };
        assert!(crate::render::to_plain(&[line(&bare, 0, &theme)]).contains("Running 1.0 s · 2 workers"));
        let cancelling = Progress { phase: Phase::Cancelling, elapsed: Duration::from_secs(4), ..Progress::default() };
        assert!(crate::render::to_plain(&[line(&cancelling, 0, &theme)]).contains("Cancelling"));
    }

    #[test]
    fn progress_from_record_reads_state_and_stage_totals() {
        let record: QueryRecord = serde_json::from_str(r#"{"id":"q","state":"RUNNING","admission_wait_ms":120,"stages":[{"task_count":4,"completed_tasks":1,"tasks":[{"node_id":"a"},{"node_id":"b"}]},{"task_count":1,"completed_tasks":0,"tasks":[{"node_id":"a"}]}],"scans":[{"rows_selected":10,"rows_emitted":7,"compressed_bytes_selected":1}]}"#).unwrap();
        let progress = Progress::from_record(&record, Duration::from_secs(1));
        assert!(matches!(progress.phase, Phase::Running));
        assert_eq!((progress.tasks_done, progress.tasks_total, progress.workers, progress.rows_scanned), (1, 5, 2, 7));
        assert_eq!(progress.query_id.as_deref(), Some("q"));
    }
```

The exact spacing of the right-aligned hint is a rendering detail: implement `line` to pad the hint to column 72 when the terminal is at least 80 wide, and assert with `contains` in the first test rather than equality (adjust the first assertion to `contains("Submitting 0.4 s")` and `ends_with("Ctrl-C to cancel\n")`).

- [ ] **Step 2: Implement `progress.rs`**

```rust
use crate::client::session::QueryRecord;
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;
use std::time::Duration;

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    #[default]
    Submitting,
    Queued,
    Running,
    Cancelling,
}

#[derive(Default, Clone, Debug)]
pub struct Progress {
    pub phase: Phase,
    pub elapsed: Duration,
    pub query_id: Option<String>,
    pub admission_wait_ms: u64,
    pub queue_ahead: Option<u64>,
    pub tasks_done: usize,
    pub tasks_total: usize,
    pub workers: usize,
    pub rows_scanned: u64,
}

impl Progress {
    pub fn from_record(record: &QueryRecord, elapsed: Duration) -> Progress {
        let phase = match record.state.as_str() {
            "QUEUED" => Phase::Queued,
            _ => Phase::Running,
        };
        let workers = record.stages.iter().flat_map(|s| s.tasks.iter().map(|t| t.node_id.as_str())).collect::<BTreeSet<_>>().len();
        Progress {
            phase,
            elapsed,
            query_id: Some(record.id.clone()),
            admission_wait_ms: record.admission_wait_ms,
            queue_ahead: None,
            tasks_done: record.stages.iter().map(|s| s.completed_tasks).sum(),
            tasks_total: record.stages.iter().map(|s| s.task_count).sum(),
            workers,
            rows_scanned: record.scans.iter().filter_map(|s| s.rows_emitted).sum(),
        }
    }
}

pub fn line(progress: &Progress, tick: usize, theme: &Theme) -> Line<'static> {
    let seconds = format!("{:.1} s", progress.elapsed.as_secs_f64());
    let mut parts = Vec::new();
    let head = match progress.phase {
        Phase::Submitting => format!("Submitting {seconds}"),
        Phase::Queued => {
            let mut text = format!("Queued {seconds} for memory admission");
            if let Some(ahead) = progress.queue_ahead {
                text.push_str(&format!(" · {ahead} ahead"));
            }
            text
        }
        Phase::Running => format!("Running {seconds}"),
        Phase::Cancelling => format!("Cancelling {seconds}"),
    };
    parts.push(head);
    if matches!(progress.phase, Phase::Running) {
        if progress.tasks_total > 0 {
            parts.push(format!("{}/{} tasks", progress.tasks_done, progress.tasks_total));
        }
        if progress.workers > 0 {
            parts.push(format!("{} worker{}", progress.workers, if progress.workers == 1 { "" } else { "s" }));
        }
        if progress.rows_scanned > 0 {
            parts.push(format!("{} rows scanned", crate::render::human_count(progress.rows_scanned)));
        }
    }
    let body = format!(" {} {}", SPINNER[tick % SPINNER.len()], parts.join(" · "));
    let hint = if matches!(progress.phase, Phase::Cancelling) { "" } else { "Ctrl-C to cancel" };
    let padding = 72usize.saturating_sub(body.chars().count()).max(2);
    Line::from(vec![
        Span::styled(body, theme.accent),
        Span::styled(format!("{}{hint}", " ".repeat(padding)), theme.dim),
    ])
}
```

- [ ] **Step 3: Rework `shell/app.rs` for the running state**

Replace `run_statement` and the `Submit` arm. `App` gains `session: SharedSession` (constructed in `run` from the `Session` — `run` takes `Session` by value now: `pub fn run(session: Session, options: &mut Options)`; update `remote::run`), `running: Option<Running>` where:

```rust
struct Running {
    handle: crate::client::statement::Handle,
    progress: Progress,
    sql: String,
    polls: u32,
    last_poll: std::time::Instant,
    cancel_requested: bool,
}
```

The loop, when `app.running` is `Some`:
- Draw the running line in a 1-row area above the editor (`Layout::vertical([Length(1), Length(editor_height), Length(1)])`), editor rendered with `running = true`.
- Every 250 ms: if `progress.query_id` is `None` and `polls < 240`, `find_query_by_tag`; else `fetch_query(id)`; update `progress` via `Progress::from_record(&record, handle.started.elapsed())`, keeping `phase = Cancelling` if `cancel_requested`. `queue_ahead` comes from `app.cluster.queue_depth()` when `Queued`.
- Non-blocking `handle.events.try_recv()`: on `Finished(result)` → emit the result through `remote::format_result` + footer (Stage 3 replaces with the styled renderer): for this task, format with `crate::output::format_rows(names, &result.data, options.output_format)` and a one-line `✓ {elapsed} · {rows} rows · query {id}` in `theme.ok`; on `Failed(f)` → emit `✗ {f.message}` in `theme.error`; then `app.running = None`, `last_elapsed_ms`/`last_scanned_rows` updated from a final `fetch_query`.
- Keys while running: Ctrl-C → if `!cancel_requested` and `query_id` is `Some`, `cancel_query` (ignore errors), `cancel_requested = true`, `phase = Cancelling`; if already requested or no id yet, abandon: emit `✗ abandoned after {elapsed}; the statement may still finish on the coordinator`, drop the handle (`app.running = None`). Every other key is ignored while running.
- `Submit` when not running: dot commands and `SHOW/USE/DESCRIBE` (via `crate::client::metadata` after Task 9; until then `remote::parse_sql_metadata`) run inline as today; SQL statements → `submit(...)` per split statement, queued in `app.pending: VecDeque<String>`, the next one starting when the current finishes.

`event::poll` timeout becomes 66 ms always (spinner tick = loop counter / 2).

- [ ] **Step 4: Manual check** — the 35 s `COUNT(DISTINCT user_id)` shows the spinner, elapsed and (at least) `2 workers`; Ctrl-C shows `Cancelling` then `✗`; the query record on `http://localhost:8081/ui` shows `CANCELED`. `-e` behaviour unchanged.

- [ ] **Step 5: Tests + gates + commit** — `git commit -m "cli: live running line with Ctrl-C cancel"`.

---

## Stage 3 — Renderers, summary, errors (and `remote.rs` retired)

### Task 9: `client/error.rs` + `render/error.rs`

**Interfaces:**
- `ErrorKind { Parse, Planning, Worker, Admission, Cancelled, Connection, Authentication, Coordinator }`
- `CliError { kind, message: String, query_id: Option<String>, workers: Vec<String>, position: Option<(usize, usize)>, sql: Option<String> }`
- `CliError::from_http(failure: CliHttp, sql: Option<&str>) -> CliError`
- `render::error::plain(&CliError) -> String` (one line, `error: <kind>: <message> (query <id>)`), `render::error::panel(&CliError, &Theme) -> Vec<Line>`.

- [ ] **Step 1: Tests**

```rust
    #[test]
    fn worker_failures_are_deduplicated_and_unwrapped() {
        let failure = CliHttp { status: Some(500), code: None, timed_out: false, connect: false, message: "worker 'worker-1' failed task with 500 Internal Server Error: {\"error\":\"storage: projection references unknown column 'nope'\"}; worker 'worker-2' failed task with 500 Internal Server Error: {\"error\":\"storage: projection references unknown column 'nope'\"}".into() };
        let error = CliError::from_http(failure, Some("SELECT nope FROM t"));
        assert_eq!(error.kind, ErrorKind::Worker);
        assert_eq!(error.message, "storage: projection references unknown column 'nope'");
        assert_eq!(error.workers, ["worker-1", "worker-2"]);
        assert_eq!(render::error::plain(&error), "error: Worker failure: storage: projection references unknown column 'nope'");
    }

    #[test]
    fn parse_errors_drop_the_transport_prefix_and_keep_a_position() {
        let failure = CliHttp { status: Some(400), code: Some("SQL_PARSE_ERROR".into()), timed_out: false, connect: false, message: "SQL parse error: sql: Expected an expression, found: FROM at line 1, column 8".into() };
        let error = CliError::from_http(failure, Some("SELECT FROM t"));
        assert_eq!(error.kind, ErrorKind::Parse);
        assert_eq!(error.message, "Expected an expression, found: FROM");
        assert_eq!(error.position, Some((1, 8)));
        let panel = render::to_plain(&render::error::panel(&error, &Theme::mono()));
        assert!(panel.contains("✗ SQL parse error"));
        assert!(panel.contains("SELECT FROM t\n          ^"));
    }

    #[test]
    fn status_codes_map_to_kinds() {
        for (status, code, kind) in [(429, Some("MEMORY_ADMISSION_REJECTED"), ErrorKind::Admission), (409, Some("QUERY_CANCELED"), ErrorKind::Cancelled), (401, None, ErrorKind::Authentication), (400, Some("PLANNING_ERROR"), ErrorKind::Planning), (503, None, ErrorKind::Coordinator)] {
            let failure = CliHttp { status: Some(status), code: code.map(str::to_owned), message: "m".into(), timed_out: false, connect: false };
            assert_eq!(CliError::from_http(failure, None).kind, kind, "{status}");
        }
        let connect = CliHttp { status: None, code: None, message: "refused".into(), timed_out: false, connect: true };
        assert_eq!(CliError::from_http(connect, None).kind, ErrorKind::Connection);
    }
```

- [ ] **Step 2: Implement** — parsing rules: split the message on `; ` when every piece starts with `worker '`; capture the worker name with the pattern `worker '<name>' failed task with <n> <text>: <json>`; take `error` from the JSON if it parses, else the trailing text; identical messages collapse; the position regex-free parse looks for ` at line <n>, column <m>` at the end and strips it; the prefixes `SQL parse error: sql: `, `planning error: `, `coordinator returned HTTP <n> <text>: ` are removed when present (the `code` field decides the kind first; the prefix is a fallback). The panel: line 1 `✗ <kind>` in `theme.error` with `query <id>` right-aligned dim; line 2 message (with `(worker-1, worker-2)` dim when workers are present); then up to three SQL lines dimmed, and a caret line when `position` is set (caret at `column - 1`, plus the width of the two-space indent).

- [ ] **Step 3: Tests pass; commit** — `git commit -m "cli: one error panel — kinds, de-duplicated workers, positions"`.

### Task 10: `render/{table,vertical,markdown,delimited,json}.rs` + `render/summary.rs`; delete `output.rs`, `display.rs`, `remote.rs`

- [ ] **Step 1: Move `output.rs` bodies** into the five files as `pub fn plain(...) -> String` with the exact code; `render/mod.rs` gains `pub enum Format` (the old `OutputFormat`, same `parse`) and `pub fn render_plain(format, names, rows, width: Option<usize>) -> String` dispatching as `format_rows` did. Move the `output.rs` tests with them. `args.rs` re-exports `render::Format as OutputFormat`.

- [ ] **Step 2: `render/table.rs::styled(names, rows, width, theme) -> Vec<Line>`** — tests first:

```rust
    #[test]
    fn styled_table_uses_box_drawing_right_aligns_numbers_and_truncates() {
        let names = vec!["region".into(), "events".into()];
        let rows = vec![vec![json!("Europe"), json!(4647390)], vec![json!(null), json!(12)]];
        let text = render::to_plain(&styled(&names, &rows, Some(40), &Theme::mono()));
        assert_eq!(text.lines().next().unwrap(), "┌────────┬───────────┐");
        assert!(text.contains("│ region │    events │"));
        assert!(text.contains("│ Europe │ 4,647,390 │"));
        assert!(text.contains("│ NULL   │        12 │"));
        let wide = vec![vec![json!("x".repeat(80)), json!(1)]];
        let text = render::to_plain(&styled(&names, &wide, Some(40), &Theme::mono()));
        assert!(text.lines().all(|l| l.chars().count() <= 40));
        assert!(text.contains('…'));
    }
```

Implementation: numeric detection = `Value::Number`; thousands via `render::thousands` for integers, `{:.4}` for floats (as `display.rs` did); column widths from content, then if the total exceeds `width`, shrink the widest string columns first to fit (minimum 8), truncating cells with `…`; a `truncated: bool` output flag (`styled` returns `(Vec<Line>, bool)`) so the summary can add the hint. Borders `┌─┬┐│├┼┤└┴┘`, header cells in `theme.accent`, `NULL` in `theme.dim`.

- [ ] **Step 3: `render/summary.rs`**

```rust
pub struct Summary { pub ok: bool, pub elapsed_ms: u64, pub rows: usize, pub total_rows: Option<usize>, pub workers: usize, pub rows_scanned: Option<u64>, pub bytes_read: Option<u64>, pub query_id: Option<String>, pub cache_hit: bool, pub admission_wait_ms: u64, pub coordinator_reason: Option<String>, pub partial_metrics: bool, pub truncated: bool, pub cancelled: bool }
pub fn from_result(result: &StatementResult, record: Option<&QueryRecord>, truncated: bool) -> Summary
pub fn lines(summary: &Summary, theme: &Theme) -> Vec<Line<'static>>
```

Test expectations (plain): `✓ 1.10 s · 5 rows · 2 workers · 18.0M rows scanned (16.4M rows/s) · 6.2 MiB read` then `  query 66aea874 · from cache` only when the second line has content; `✗ cancelled after 4.10 s`; `1,000 of 84,312 rows · Space/Enter for more · q to stop` when `total_rows > rows`.

- [ ] **Step 4: Wire the shell** — `Finished` now renders through `render_styled` (table → `table::styled`, vertical/markdown → their plain text as `Line::raw`, machine formats → plain) followed by `summary::lines`; `Failed` → `error::panel`. Batch (`batch.rs`) uses `render_plain` + `render::error::plain` on stderr and keeps today's footer text (`Query <id> <state> in <ms> ms (...)` + telemetry lines) — move `format_query_telemetry` from `remote.rs` to `render/summary.rs` as `pub fn plain_footer(...)` unchanged so `-e` output stays identical.

- [ ] **Step 5: Retire `remote.rs`** — metadata parsing moves to `client/metadata.rs` with its tests (Task 11 uses it); `execute`/`execute_to_string`/pager handling move to `batch.rs` (pager stays for batch mode only when stdout is a TTY — same condition as today). Delete `remote.rs`, `output.rs`, `display.rs`.

- [ ] **Step 6: Gates; `cli_workflows.py` passes unchanged; commit** — `git commit -m "cli: styled tables and summaries; plain renderers unchanged; remote.rs retired"`.

---

## Stage 4 — Paged results

### Task 11: `client/pages.rs` and the result pager in the shell

**Interfaces:**
- `PageCursor::new(session: SharedSession, next_uri: String) -> PageCursor`; `fetch_next(&mut self) -> Result<Option<Page>, CliHttp>` where `Page { rows: Vec<Vec<Value>>, next_uri: Option<String> }`; `PageCursor::validated(server, uri)` refuses a `next_uri` whose scheme/host/port differ from the server (like the Trino runner does).

Inspect `engine/crates/server/src/results.rs::page` for the page JSON shape before writing the deserializer (it returns the page's rows and the following URI; copy the field names from `page()`).

- [ ] **Step 1: Tests** — fixture serving `/v1/query/q/results/1` then `/2`; cursor yields two pages then `None`; a `next_uri` on another host is refused with `CliHttp.message` containing `unsafe next URI`.

- [ ] **Step 2: Shell integration** — after `Finished`, if `result.next_uri` is `Some`, enter `Paging { cursor, rendered_rows, total }` state: the summary's third line reads `1,000 of ? rows · Space/Enter for more · q to stop` (`?` until the record's total is known — `QueryRecord` gains `rows_are_preview: bool` and `row_count: Option<usize>` if `results.rs` publishes one; otherwise the total stays unknown and the line reads `1,000 rows so far`); Space/Enter fetches and renders the next page as its own table (header repeated), `q`/Esc stops; the editor is inactive until then.

- [ ] **Step 3: `--paged` for batch** — `batch.rs` sends `result_delivery: paged` when set and streams every page through the plain renderer (header once, rows appended; for `csv`/`tsv`/`json` lines simply concatenated; for `json` (pretty array) collect all pages first).

- [ ] **Step 4: Manual check** — `SELECT * FROM kaveon_events_users LIMIT 5000;` pages in the shell; `kaveon … --paged -e "SELECT * FROM kaveon_events_users LIMIT 100000" --output-format CSV | wc -l` → 100000.

- [ ] **Step 5: Commit** — `git commit -m "cli: paged results in the shell and --paged for scripts"`.

---

## Stage 5 — Commands, completion, highlighting, vi

### Task 12: `EXPLAIN`, `.cluster`, `.settings`, `.format`, `.history`, grouped `.help`; SQL highlighting; completion; vi mode

- [ ] **Step 1: `shell/commands.rs`** — `pub enum Command { Cluster, Settings(Option<(String, String)>), SettingsReset, Format(String), History(usize), Help, Clear, Quit }`, `Command::parse(text) -> Option<Result<Command, String>>` (None when not a dot command; `Err` for a bad argument). `.settings` keys and validation: `memory` (bytes with `KiB|MiB|GiB` suffix, ≥ 1), `parallelism` (1..=1024), `cache` (`on|off`), `admission_wait` (seconds, ≤ 86400). They are stored on `App.settings: serde_json::Map` under the server's names (`query_memory_limit_bytes`, `local_parallelism`, `result_cache`, `admission_wait_seconds`) and sent on every `StatementRequest.settings`. `.settings` with no arguments prints the map; `.settings reset` clears it. Tests for parse/validate.

- [ ] **Step 2: `EXPLAIN`** — in the submit path, a statement whose first token is `EXPLAIN` (case-insensitive, unquoted) strips it, submits the remainder with `settings.result_cache = false` merged in, and on `Finished` renders `render::plan::tree(record.plan["logical"])` instead of rows: each node as `<indent>└ <operator> <attributes as k=v, dim>`; test with a three-node JSON tree. If `plan.logical` is null: `plan unavailable for this statement`.

- [ ] **Step 3: `.cluster` panel** — `render::cluster::panel(&Cluster, now, theme)`: one line per node `node_id · role · v · heartbeat 3 s ago · rss 22.1 MiB · limit 4.0 GiB`, then coordinator `admission` and `result_cache` facts. Test with two nodes.

- [ ] **Step 4: Completion** — `shell/complete.rs`: `Completer { keywords: &'static [&str], names: NameCache }`; `NameCache` (in `client/metadata.rs`) lazily fetches catalogs → schemas → tables → columns (`/v1/catalog`, `/v1/catalog/{c}/schema`, `/v1/catalog/{c}/schema/{s}/table`, and the definitions routes for columns as `DESCRIBE` does today) for the session catalog/schema on first Tab, cached until `USE`. Tab in the editor: the word before the cursor is completed if unique, else a popup (a `List` widget rendered above the editor, max 8 rows, Up/Down/Tab cycles, Enter/Esc closes) — the popup takes one extra row of the viewport. Keywords: the SQL keywords the engine's parser accepts (`SELECT FROM WHERE GROUP BY ORDER LIMIT OFFSET HAVING JOIN LEFT RIGHT FULL INNER CROSS ON AS AND OR NOT IN IS NULL DISTINCT COUNT SUM AVG MIN MAX CASE WHEN THEN ELSE END CAST UNION INTERSECT EXCEPT SHOW CATALOGS SCHEMAS TABLES COLUMNS DESCRIBE USE EXPLAIN SET SESSION`) plus the dot commands.

- [ ] **Step 5: Highlighting** — in `Editor::widget`, tokenize the buffer with `sqlparser`'s `Tokenizer` and build a per-line `Vec<Span>` via `tui-textarea`'s styled-line support is not available, so implement highlighting by rendering the editor ourselves: a `Paragraph` built from tokens (keywords in `theme.accent` bold, strings in green, numbers in the default colour, comments dim) with `tui-textarea` still owning the text and cursor — draw the `Paragraph` first, then overlay the cursor position with `frame.set_cursor_position` from `area.cursor()`. Snapshot test with `TestBackend` at 60 columns.

- [ ] **Step 6: vi** — map keys through the vi state machine from `tui-textarea`'s `vim` example (insert/normal/visual; `Esc` to normal; `i a o` to insert; `Enter` in normal mode submits when complete). Only when `options.editing_mode == "VI"`.

- [ ] **Step 7: Grouped help** — `render::help(theme)` produces four short groups (Connection, Catalog, Session, Output) that fit 24 rows; `.help` emits it; `--help` text (`print_usage`) reorganised the same way. Fix the `.describe` line ("local mode" removed).

- [ ] **Step 8: Tests, gates, manual pass, commit per step** (`cli: EXPLAIN and .cluster`, `cli: session settings`, `cli: completion and highlighting`, `cli: vi mode`, `cli: grouped help`).

---

## Stage 6 — `--local` through the shell

### Task 13: `local/` produces `StatementResult`

- [ ] Move `planner.rs` → `local/planner.rs`, `config.rs` → `local/config.rs`, `build_local_catalog` and the SHOW/USE/DESCRIBE handling from `main.rs` → `local/catalog.rs`. `local/mod.rs::run(options) -> Result<(), String>` builds the `CatalogManager` as `run_local` does today, then: batch when non-TTY/`-e` (rows through `render_plain`), else `shell::run_local(catalog, options)` — a variant of the app whose statement executor is a closure `Box<dyn FnMut(&str) -> Result<StatementResult, CliError>>` converting Arrow batches to JSON rows (`batches_to_json` — port the server's `batches_to_json` shape: `Value::Number` for ints/floats, strings, bools, `Null`), with no query id and a running line that shows elapsed only. The header's Engine line reads `embedded · <data dir or config path> · <n> tables`; Cluster line omitted.

- [ ] Tests: `local::catalog` discovery tests moved from `main.rs`/`config.rs`; a `StatementResult` from a one-row Parquet fixture (`engine/testdata` — check for an existing small Parquet file; else write one with `arrow`/`parquet` in the test).

- [ ] Commit — `cli: --local runs through the same shell and renderers`.

---

## Stage 7 — Remaining server additions (while Codex is away)

### Task 14: Live task counters on the query record

**Files:** `engine/crates/server/src/api.rs` — where a distributed task's completion is recorded into the stage telemetry at statement end (search `stages: Vec<StageTelemetry>` assignments and the task-result collection in the fragment dispatch path).

- [ ] Find the point where each task's result arrives at the coordinator (`orchestrator.rs`/`api.rs` `TaskDispatch` completion) and, under the `QUERY_STORE` write lock, update the running record's `stages[stage].completed_tasks`, push the `TaskTelemetry` and add the task's scan metrics to `scans` (the same aggregation the final path does — factor it into `fn merge_task_into_record(record: &mut QueryRecord, stage_id, task: TaskTelemetry)` used by both). Test: a statement with two stages whose tasks complete one at a time (the existing distributed test harness with an injected slow worker, or a direct unit test of `merge_task_into_record`).
- [ ] `docs/reference/api.md`: note that `stages`/`scans` are updated as tasks complete while the state is `RUNNING`.
- [ ] Commit — `server: the query record reports task completion and scan counters while running`.

### Task 15: Parse-error positions

- [ ] In the `submit_statement` path where `sql_to_logical_plan` errors become `SQL_PARSE_ERROR` responses: `sqlparser::parser::ParserError` messages end with ` at Line: N, Column: M` — parse that (or use the tokenizer's `Location` where available) into `"position": {"line": N, "column": M}` on the JSON error body. Test: `SELECT FROM t` returns 400 with `position.line == 1`. Doc entry. Commit — `server: parse errors carry a position`.

---

## Stage 8 — Docs, qualification, release

### Task 16: Documentation and the packaged-binary contract

- [ ] `docs/guides/engine-cli.md`: rewrite the interactive section (header, editor, running line, cancel, paging, `EXPLAIN`, `.cluster`, `.settings`, `.format`, `.history`, `--theme`, `--no-header`, `--width`, `--paged`, new `--timeout` semantics); keep the batch/local sections accurate.
- [ ] `docs/engineering/cli-compatibility.md`: rows for live progress (supported: state/elapsed/admission, tasks and scans as reported), cancel, paging/spooling, session properties (`.settings`), `EXPLAIN`.
- [ ] `docs/engine/settings.md` CLI variables; `docs/reference/api.md` already done.
- [ ] `engine/qualification/cli_workflows.py`: add a `--paged` case (fixture serves `next_uri` + two pages) and a whoami-404 tolerance case; wire the script into `.github/workflows/engine.yml` after the release build of each target (`python engine/qualification/cli_workflows.py --cli <artifact>`).
- [ ] `node ../scripts/validate-docs.mjs` passes.
- [ ] Commit — `docs: the 0.3.0 client`.

### Task 17: Merge

- [ ] `git rebase dev` on `cli-overhaul` (the engine session merges to `dev` continuously; resolve only in `server/src` if their changes touched the same lines; the CLI crate is untouched by them).
- [ ] Full gates from `engine/`: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- [ ] Manual pass list from the spec against the Compose stack; `-e` outputs diffed against a 0.2.0 binary for `SHOW TABLES`, a `SELECT`, `CSV_HEADER`, `JSON`.
- [ ] Merge to `dev` (fast-forward or merge commit), HANDSHAKE Log row: what shipped, the three server additions, test counts. Push follows the architect's instruction.

---

## Self-review notes

- Spec coverage: header (T4/T5), editor/status (T5), running line + cancel (T7/T8), paging (T11), styled tables/summary/errors (T9/T10), commands (T12), completion/highlight/vi (T12), `--local` (T13), server additions (T2/T14/T15), flags (T5/T11), tests and docs (each task, T16), version (T1). `--disable-auto-suggestion` no-op: T5 Step 1.
- Types: `CliHttp` (T3) is consumed by T7/T9; `StatementResult`/`Column` (T7) by T10/T11/T13; `QueryRecord` (T3) by T8/T10; `Format` replaces `OutputFormat` in T10 with a re-export so `args.rs` tests keep compiling.
- Known judgement calls left to the implementer: exact padding of the running-line hint; the page-JSON field names (read `results.rs`); the `test_state` helper name in `api.rs` tests.
