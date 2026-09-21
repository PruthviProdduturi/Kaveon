# Kaveon CLI overhaul — design

Date: 2026-09-17 · Owner: Claude (ownership of `engine/crates/cli` moves from
Codex to Claude in HANDSHAKE.md with this spec) · Status: approved by the
architect, implementation next.

## Goal

Make `kaveon` — the client binary for a Kaveon coordinator — feel like a
current, well-made terminal tool (the bar set by GitHub Copilot CLI and Claude
Code): a small informative header, a pinned multi-line SQL editor with a status
bar, live feedback while a statement runs, cancel with Ctrl-C, results that
read well, errors that say one thing once. Non-interactive use (`-e`, `-f`,
piped stdin, redirected stdout) is byte-for-byte unchanged and gets none of
the decoration.

The product name in the client is **Kaveon**, one word. No "CLI", "DB" or
"Engine" variants in headers, prompts or messages.

## Non-goals (this round)

- Full-screen / alternate-screen application, result grid sorting, saved
  queries, a catalog tree pane.
- Server changes. Everything below runs against the current `/v1` API; the
  two Codex requests (below) are additive and the client degrades without them.
- Conhost quirks below Windows Terminal; the CLI targets Windows Terminal,
  macOS Terminal/iTerm, and any VT-capable Linux terminal.
- Trino wire-protocol or session-property parity.

## Server facts the design rests on (verified in `engine/crates/server/src`)

- `POST /v1/statement` is synchronous: the response carries the whole inline
  result or, with `result_delivery: "paged"`, a `next_uri` and pages at
  `GET /v1/query/{id}/results/{page}` (1,000 rows or 4 MiB per page, 15 min
  TTL, owner-scoped). Inline results over 16 MiB are refused.
- The query record is inserted into the coordinator's history **at
  submission** (state `QUEUED` while waiting for memory admission, else
  `RUNNING`), with the request's `client_tags`; `GET /v1/query` lists the
  caller's records; `GET /v1/query/{id}` returns state, `admission_wait_ms`,
  `elapsed_ms`, `execution.mode` (`distributed`/`coordinator`/`cache`),
  `plan.logical`, `stages`/`scans` (guaranteed complete only at completion),
  and `settings`.
- `DELETE /v1/query/{id}` cancels a queued or running statement.
- `GET /v1/cluster` returns coordinator and worker nodes with version,
  environment, `last_heartbeat`, memory and admission statistics.
- `GET /v1/auth/config` is public; the security layer accepts static
  principal tokens, the bridge token, Entra bearer tokens, or nothing in
  insecure-development mode. The server does not tell a client who it is.
- Per-request `settings` (`query_memory_limit_bytes`, `local_parallelism`,
  `result_cache`, `admission_wait_seconds`) and a leading `SET SESSION k = v;`
  prefix are both accepted.

## Architecture

Two threads, one channel, no async runtime in the client.

```
main ── args/config ── auth::Session::connect ──┐
                                                ├─ non-TTY: batch::run  (plain renderers, today's behaviour)
                                                └─ TTY:     shell::run  (ratatui inline viewport)
                                                                │
                                   client::Statement ───────────┤  worker thread: POST /v1/statement
                                   client::Progress  ───────────┤  UI thread: poll /v1/query by tag → /v1/query/{id}
                                   client::Pages     ───────────┘  on demand: /v1/query/{id}/results/{n}
```

### Modules (`engine/crates/cli/src`)

| Module | Responsibility | Status |
|---|---|---|
| `args.rs` | argument parsing, config-file defaults | kept; new flags below |
| `auth.rs` | HTTP client, TLS, Azure CLI / device sign-in, bearer | kept |
| `input.rs` | statement splitter, dollar quotes, comments | kept (rustyline parts removed) |
| `client/session.rs` | typed wrappers over `/v1`: cluster, whoami, catalog metadata, query record, cancel | new |
| `client/statement.rs` | submit on a worker thread with a unique tag; `StatementEvent` channel (`Submitted`, `Found{id}`, `State{..}`, `Finished{result}`, `Failed{error}`) | new |
| `client/pages.rs` | paged result cursor: fetch page *n* on demand, cache fetched pages | new |
| `client/metadata.rs` | SHOW/USE/DESCRIBE parsing (moved from `remote.rs`), name cache for completion | moved |
| `client/error.rs` | `CliError { kind, message, query_id, sql, position }`; de-duplicates worker failures, unwraps JSON, strips transport prefixes | new |
| `render/table.rs`, `vertical.rs`, `markdown.rs`, `delimited.rs`, `json.rs` | result formats; each has `plain()` (today's bytes) and `styled()` (TTY) | `output.rs` split |
| `render/summary.rs` | the two-line query summary | new |
| `render/error.rs` | the error panel (styled) and the one-line form (plain) | new |
| `render/plan.rs` | indented plan tree from `plan.logical` | new |
| `render/cluster.rs` | header lines and `.cluster` panel | new |
| `shell/app.rs` | event loop: keys, statement events, redraw at ≤ 15 fps | new |
| `shell/editor.rs` | `tui-textarea` wrapper, SQL highlighting, submit rules, history navigation | new |
| `shell/status.rs` | editor box title and bottom status line | new |
| `shell/progress.rs` | running line: spinner, elapsed, state, admission, tasks | new |
| `shell/complete.rs` | keyword + catalog name completion popup | new |
| `theme.rs` | one accent, dim, warning, error; `NO_COLOR`, `TERM=dumb` → plain | new |
| `batch.rs` | `-e` / `-f` / stdin path (today's `execute_script`) | from `remote.rs` |
| `local/` (`planner.rs`, `config.rs`, catalog discovery) | embedded execution behind `--local`, now producing `Result` rows for the same renderers and shell | moved |

`remote.rs`, `display.rs` and `main.rs`'s stdin REPL disappear.

### Dependencies added

`ratatui` (inline viewport), `crossterm`, `tui-textarea`, `unicode-width`,
`uuid` (v4, already a workspace dependency). `rustyline` and `terminal_size`
are removed. No tokio.

## Behaviour

### Startup and header (TTY only)

1. `Session::connect` as today (auth discovery, token).
2. `GET /v1/cluster` (30 s metadata timeout). `GET /v1/whoami` when it
   exists; a 404 hides the identity field.
3. Header, printed once into scrollback:

```
  KAVEON  v0.3.0
  ─────────────────────────────────────────────────────────────
  Engine    http://localhost:8081  ·  v0.1.0  ·  docker
  Cluster   coordinator-1  ·  2 workers ready  ·  4.0 GiB admission
  Session   prproddu  ·  admin  ·  auth none (insecure development)

  SQL ends with ;   .help for commands   Ctrl-C cancels a running query
```

- "workers ready" counts heartbeats within the last 30 s; otherwise
  `2 workers · 1 stale`, in the warning colour. Zero workers: `no workers —
  statements run on the coordinator`, warning colour.
- Identity line: `<user> · <role>` from `/v1/whoami`; without it, `<--user>`
  and the auth mode only.
- Version comes from the cluster payload, never assumed.

### Editor and status bar (pinned, inline viewport)

```
┌ OpenSource.kaveon_product ───────────────────────────────────────────┐
│ SELECT region, COUNT(*) FROM kaveon_events_enriched                  │
│ GROUP BY region;▌                                                    │
└──────────────────────────────────────────────────────────────────────┘
 localhost:8081 · 2 workers · last 1.10 s · 18.0M rows scanned
```

- Box title = session catalog.schema; changes with `USE`/`.use`.
- Editor grows with content up to a third of the terminal height, then
  scrolls internally.
- Enter submits when the buffer, minus trailing whitespace and comments,
  ends with `;`; otherwise inserts a newline. Ctrl-Enter (or Alt-Enter where
  the terminal cannot distinguish) submits regardless. Several statements in
  the buffer run in order.
- Up/Down browse history when the cursor is on the first/last line; Ctrl-R
  searches history. History file location and format unchanged.
- Tab completion: SQL keywords, dot commands, and catalog/schema/table/column
  names fetched lazily (`client/metadata.rs` cache, invalidated by `USE`).
- Emacs bindings default; `--editing-mode vi` gives vi insert/normal modes
  via `tui-textarea`.
- Ctrl-C with an empty editor and nothing running: prints "Ctrl-D or .quit
  to exit"; Ctrl-D / `.quit` / `exit` leave, saving history.
- Status line: server host, worker count, last statement duration and scanned
  rows; dimmed. Turns to the warning colour when the last cluster poll (every
  30 s, cheap) shows a stale worker.

### Running a statement

1. The editor box dims and shows the SQL being run; the progress line
   appears above the box:

```
 ⠸ Running 2.4 s · 3/5 tasks · 2 workers · 210M rows scanned          Ctrl-C to cancel
```

   - Before the record is found: `⠸ Submitting …`.
   - `QUEUED`: `⠸ Queued 3.2 s for memory admission · 2 ahead` (queue depth
     from the cluster payload when the record does not carry it).
   - Task and scan counters are shown only when the record reports them
     (`> 0`); until then the line shows elapsed, state and workers. This is an
     honest limitation of the current server; see the Codex request.
2. The worker thread posts with `client_tags: [..user tags, "kaveon-cli:<uuid>"]`
   and `result_delivery: "paged"`.
3. The UI thread polls `GET /v1/query` every 250 ms until a record with the
   tag appears (bounded to 60 attempts; after that the line reads
   `Running … · id pending`), then `GET /v1/query/{id}` every 250 ms.
4. Ctrl-C → `DELETE /v1/query/{id}`; the line reads `Cancelling …`; when the
   POST returns (409/`QUERY_CANCELED` or an error) the summary says
   `✗ cancelled after 4.1 s`. A second Ctrl-C abandons the wait and returns
   the editor; the statement may still finish on the server.
5. Client timeouts: statements use `--timeout` (default now 24 h, applying to
   connect and read stalls, not to duration); metadata calls 30 s.

### Results

- Interactive: pages are rendered as they are fetched; the first page prints
  immediately with the summary; when a result has more pages the summary
  says `1,000 of 84,312 rows · Space/Enter for more · q to stop`, and the
  editor is inactive until the cursor is released. `--output-format-interactive`
  still selects the renderer.
- Tables (`styled()`): light box-drawing borders, header in the accent
  colour, numbers right-aligned with thousands separators (integers) and
  fixed 4-decimal floats as today, `NULL` dimmed, strings left-aligned.
  Columns wider than the terminal are truncated with `…` and the summary adds
  `some columns truncated · .format vertical`. `AUTO` switches to vertical
  when even truncated columns do not fit.
- `plain()` renderers keep today's exact output; the existing tests pin
  them. `-e`/`-f` keep `result_delivery: inline` unless `--paged`.
- Summary, two lines:

```
 ✓ 1.10 s · 5 rows · 2 workers · 18.0M rows scanned (16.4M rows/s) · 6.2 MB read
   query 66aea874 · from cache          (second line only when there is something to say)
```

  Second-line facts: `from cache`, `waited 3.2 s for admission`, `ran on the
  coordinator: <detail>`, `partial worker metrics`. JSON bytes-per-second is
  gone. The summary is written to scrollback; `NULL` format prints nothing.

### Errors

One panel, in scrollback:

```
 ✗ Worker failure                                          query 7c0c9b9e
   storage: projection references unknown column 'nope'  (worker-1, worker-2)
   SELECT nope FROM kaveon_events_users
          ^^^^
```

- Kinds: `SQL parse error`, `Planning error`, `Worker failure`,
  `Memory admission`, `Cancelled`, `Connection`, `Authentication`,
  `Coordinator` (anything else). Mapping from HTTP status + `code` field.
- Worker failures: identical messages collapsed to one line with the set of
  workers; JSON bodies unwrapped.
- The caret line appears only when the server supplies a position (it does
  not today); otherwise the SQL excerpt is shown without one, first 3 lines.
- Plain form (non-TTY): `error: <kind>: <message> (query <id>)` on stderr,
  exit codes unchanged.

### Commands

| Command | Effect |
|---|---|
| `SHOW CATALOGS/SCHEMAS/TABLES [IN …] [LIKE …]`, `DESCRIBE`, `SHOW COLUMNS`, `USE` | as today, client-side over `/v1/catalog` |
| `EXPLAIN <statement>` | runs the statement with `result_delivery: paged` and `settings.result_cache=false`, discards rows, renders `plan.logical` as an indented tree plus the summary |
| `.cluster` | cluster panel: nodes, versions, heartbeat age, memory/admission, result cache |
| `.settings` / `.settings <key> <value>` / `.settings reset` | shows or sets the session's `settings` object (keys `memory`, `parallelism`, `cache`, `admission_wait`); values validated locally against the same ranges the server enforces, sent with every statement |
| `.format <name>` | change the interactive result format |
| `.history [n]` | last *n* statements with duration and status |
| `.help`, `.clear`, `.quit` | as today; help is grouped (Connection, Catalog, Session, Output) and fits one screen |
| `SET SESSION … ;` prefix | passed through unchanged |

### `--local`

Same shell, renderers, summary and errors. `local/` exposes the same
`Statement`-shaped result (`columns`, `rows`, `elapsed`, no query id, no
progress polling — the running line shows elapsed only). Local catalog
discovery and config parsing are unchanged.

### Flags

Added: `--paged` (batch mode: page large results instead of failing at
16 MiB), `--no-header`, `--theme <dark|light|mono>` (default dark; `mono` =
no colour), `--width <cols>` (override terminal width; `COLUMNS` honoured).
Changed: `--timeout` default 24 h for statements (documented; metadata calls
stay 30 s). Removed: `--disable-auto-suggestion` (history hints are part of
the completion popup; keep the flag accepted as a no-op for one release).

## Server additions (Claude, on the architect's authorization while Codex is away until 2026-09-19)

Three additive changes in `engine/crates/server`, recorded in HANDSHAKE.md as a
boundary crossing. The client degrades without each one so an older
coordinator keeps working.

1. `GET /v1/whoami` → `{ "principal": "...", "display": "...", "role": "reader|analyst|admin", "auth": "static|bridge|entra|development" }` for the authenticated identity, from the `Identity` the security layer already attaches. The header shows `<display or principal> · <role>`; 404 hides the line.
2. Live task counters on `GET /v1/query/{id}` while `RUNNING`: `stages[].completed_tasks` and `scans[].rows_emitted` updated as workers finish tasks (the per-task metrics already exist; publish them into the record on each task's completion instead of at the statement's end). Improves the running line; nothing else depends on it.
3. Error positions: `{ "error": ..., "code": "SQL_PARSE_ERROR", "position": { "line": 1, "column": 8 } }` on parse errors (sqlparser reports a location) and, where the binder has one, binding errors. Enables the caret.

Each ships with its own tests and a `docs/reference/api.md` entry; none
changes an existing field.

## Testing

- Unit: renderers (`plain()` bytes pinned by the existing tests; `styled()`
  snapshot strings with ANSI stripped), summary/error formatting, tag
  generation, `.settings` validation, metadata parser (moved tests).
- Protocol: a `TcpListener` fixture in `client/` tests (pattern already in
  `remote.rs`/`auth.rs`): record found by tag after two polls, cancel sends
  `DELETE`, paged cursor fetches page 2 on demand and caches it, whoami 404.
- Shell: `ratatui::backend::TestBackend` snapshots for header, idle editor,
  running line (submitting/queued/running), summary, error panel, at 80 and
  120 columns.
- Packaged binary: `engine/qualification/cli_workflows.py` extended with the
  paged path and the `--paged` flag; run in the Engine workflow after the
  release build.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, a manual pass against the local
  Compose stack (`docs/guides/local-stack.md`) covering: header with 2
  workers, a 30 s statement with Ctrl-C, a 100k-row paged result, a worker
  error, `EXPLAIN`, `.settings memory 256MiB`, `--local` on `tmp/kaveon-events`,
  `-e` output identical to 0.2.0.

## Documentation to update with the code

`docs/guides/engine-cli.md` (rewrite the interactive section), 
`docs/engineering/cli-compatibility.md` (progress, cancel, paging rows),
`docs/engine/settings.md` (CLI variables), `engine/crates/cli/Cargo.toml`
version → `0.3.0`, HANDSHAKE ownership row and Log.
