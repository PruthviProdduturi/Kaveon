# CLI compatibility checkpoint

This compares the `kaveon` client (0.3.0) with the everyday workflows
documented in the [Trino CLI guide](https://trino.io/docs/current/client/cli.html).
It is not a claim of complete CLI, wire-protocol or SQL-engine parity. The
[CLI guide](../guides/engine-cli.md) is the reference for what each row
describes.

| Workflow | Kaveon status |
|---|---|
| Connect to HTTPS with Azure login | Supported; the Engine-scoped token comes from the Azure CLI login or Microsoft device sign-in and is renewed in process |
| Connection invocation and defaults | Positional coordinator URL with optional `/catalog/schema`, `--option=value`, duration `--timeout`, and an allowlisted `KAVEON_CONFIG` / `~/.kaveon_config` defaults file |
| Explicit bearer token | `--access-token`; `KAVEON_ACCESS_TOKEN` is preferable for unattended use |
| Session header | Engine URL, coordinator version and environment, workers ready or stale, admission limit, identity and role from `GET /v1/whoami` (hidden on older coordinators), the auth mode with an insecure-development note |
| `SHOW CATALOGS` | Client metadata command over the authenticated catalog API; singular `SHOW CATALOG` accepted |
| `SHOW SCHEMAS IN/FROM catalog` | Client metadata command |
| `SHOW TABLES IN/FROM [catalog.]schema` | Client metadata command |
| `SHOW ... LIKE 'pattern'` | Client-side `%` / `_` filtering over authenticated metadata names |
| `DESCRIBE` / `SHOW COLUMNS FROM` | Catalog-definition metadata: column name, type and nullability |
| `USE [catalog.]schema` | Validates the target before switching; a bare name resolves to a schema of the current catalog or to a catalog; the context shows on the status line |
| Unknown names | `did you mean` for SHOW kinds, dot commands, `.settings` keys, `USE` targets; a missing table is located across catalogs and the `USE` to reach it is given |
| `.catalogs`, `.schemas`, `.tables`, `.describe`, `.use` | Metadata shortcuts without a semicolon |
| `help`, `exit`, `quit`, `clear` | Shell commands, optional semicolon; `clear` needs an interactive terminal |
| Multi-line editing | Enter submits only a statement ending in `;` (Ctrl-Enter forces), Up/Down history filtered by the typed prefix with the draft kept, Ctrl-R reverse history search, an inline suggestion from history taken with Right, readline keys (Ctrl-A/E/K/U/W, Alt-B/F, Alt-Z/Y undo and redo), Tab completion of keywords, dot commands and catalog names, SQL highlighting, pasted text inserted verbatim and never run |
| `.ask` | A plain-language question through the Kaveon DLM on the platform API (`--api`); Live answers over a native catalog run on the coordinator; clarifications are answered with `.ask <n>`. Trino has no equivalent |
| `.source`, `.tee`, `.edit`, `\G`, `.watch` | Run a file through the shell, copy the scrollback to a file, edit the last statement in `$EDITOR`, vertical output for one statement, re-run on an interval |
| `--editing-mode VI` | Accepted; applies to the line-editor fallback when stdout is not a terminal, not to the shell's editor |
| Live progress | Supported: the running line shows state (`Submitting`, `Queued … for memory admission · N ahead`, `Running`, `Cancelling`), elapsed time, admission wait, tasks done/total, distinct workers and rows scanned exactly as the coordinator's query record reports them, polled every 250 ms by the statement's client tag |
| Cancel | Ctrl-C sends `DELETE /v1/query/{id}` and waits for the coordinator; a second press abandons the wait. `.queries` lists live statements and `.kill <id>` cancels any of them |
| Query summary | One line: elapsed time, rows, distinct worker nodes, rows scanned with rate, compressed bytes read, query id; a second line only for cache hits, coordinator fallback, incomplete tasks, admission wait, partial metrics, the row-limit or truncation note |
| Result tables | Styled box table in the shell (right-aligned numbers with thousands separators, dimmed `NULL`, narrowed string columns with `…`, `AUTO` falls back to vertical); the plain aligned form for scripts is unchanged |
| Paging | Queries without `LIMIT` show the first 1,000 rows (`.limit`, `--row-limit`); longer results are shown a page at a time; scripts are unlimited and take `--paged` to fetch result pages past the 16 MiB inline ceiling |
| `EXPLAIN` | Runs the statement with the result cache off and prints the query record's logical plan as an indented tree; there is no `EXPLAIN ANALYZE` and no physical or distributed plan |
| Session properties | `.settings` with `memory`, `parallelism`, `cache`, `admission_wait`, validated in the client and sent as the request's `settings` with every statement; a leading `SET SESSION k = v;` passes through unchanged. There is no server-side session, so Trino's `SET SESSION` catalogue is not mirrored |
| `--execute` and `--file` | Supported; a UTF-8 script runs statements in order; `--ignore-errors` continues after failures with a nonzero exit; failures are one line, `error: <kind>: <message> (query <id>)` |
| Persistent history | `%APPDATA%\kaveon\history` or `~/.kaveon_history`; `--history-file`, `KAVEON_HISTORY_FILE`, `--no-history`; multi-line statements kept whole |
| Output formats | Legacy lowercase `table`, `csv`, `tsv`, `json`, plus uppercase `ALIGNED`, `VERTICAL`, `AUTO`, `MARKDOWN`, the CSV/TSV variants, JSON Lines and `NULL`; `.format` changes it in the shell |
| Appearance | `--theme` (`dark`, `light`, `mono`), `NO_COLOR`, `TERM=dumb`, `--no-header`, `--width` |
| External pager | `--pager` / `KAVEON_PAGER` for scripts and the line-editor fallback printed to a terminal; empty disables it |
| Scan-reader counters | Rows emitted, selected row-group rows and selected compressed bytes from complete Parquet worker coverage; incomplete coverage is labelled `partial worker metrics` rather than shown as zero |
| Streaming rows mid-statement | Not supported: `POST /v1/statement` returns when the statement finishes, so rows appear at the end; the running line covers the wait |
| Spooling to object storage | Not supported; results are inline or paged from the coordinator's result store (15 min TTL) |
| Kerberos, JWT files, `--password`, external authentication | Not supported; the client's modes are `auto`, `azure-cli`, `microsoft`, `none` with a static bearer token |
| Full Trino session properties, `--client-info`, resource estimates, wire protocol | Not claimed; requires separate Engine and client work |

`SHOW`, `DESCRIBE` and `USE` are interpreted by the client. The Engine SQL
HTTP endpoint has its own supported SQL surface; see
[Engine SQL compatibility](../reference/engine-sql-compatibility.md).
Unsupported metadata clauses report errors instead of being silently
ignored. A failed context change keeps the previous catalog and schema, and
an interactive error keeps the session open.

Human-readable output ends with a blank line before the next prompt.
Lowercase machine formats retain their existing serialisation. Uppercase
`JSON` emits one JSON object per row and `NULL` emits no result rows. `AUTO`
uses `COLUMNS` when available and otherwise assumes 120 columns before
choosing aligned or vertical output. These names support common Trino CLI
workflows but do not claim exact Trino formatter byte compatibility. The
summary reads the query record after execution; a missing or expired record
does not turn a successful query into an error. Workers in the summary are
the distinct nodes that ran tasks, not every node in the cluster. Metadata
commands end with `catalog API` and never fabricate a query id or execution
statistics.

The Engine UI labels queries using their reported client metadata (for
example `kaveon-cli` becomes **Kaveon CLI**). Its **User** field comes from
the authenticated server identity, with an immutable principal retained
separately for ownership and audit. A submitted CLI username does not grant
access or replace that identity.
