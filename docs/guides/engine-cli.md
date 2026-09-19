# Kaveon CLI

Status: **Beta** for the remote shell; embedded `--local` mode is **alpha**.

`kaveon` is the terminal client for a Kaveon Engine coordinator. It connects
over HTTPS, runs SQL through `POST /v1/statement`, answers catalog questions
over the catalog API, and shows what the coordinator reports about each
statement while it runs. Non-interactive use (`-e`, `-f`, piped input,
redirected output) prints plain text and is scriptable; the interactive
shell adds the session header, the editor, the running line and the styled
results described below.

This page is the client reference. The [local stack guide](local-stack.md)
starts a coordinator and two workers on your machine; the
[compatibility checkpoint](../engineering/cli-compatibility.md) lists what is
and is not covered relative to the Trino CLI.

## Install

Versioned releases are tagged `cli-vX.Y.Z` on the
[releases page](https://github.com/PruthviProdduturi/Kaveon/releases) and
built by the [CLI release workflow](../../.github/workflows/cli-release.yml)
for Windows x64, macOS (Apple Silicon and Intel) and Linux x64. Each release
carries a `SHA256SUMS` file; the install scripts and the package managers
verify against it. The moving `engine-dev` preview is built from every push
to `dev` by the [Engine workflow](../../.github/workflows/engine.yml) and is
not checksummed. The release procedure is in
[Cutting a CLI release](../engineering/cli-release.md).

### Package managers

Not needed, and not published. The release assets include a rendered winget
manifest and a Homebrew formula so either can be submitted later — winget
through a pull request to `microsoft/winget-pkgs`, Homebrew through
homebrew-core once the project qualifies — but the install scripts and the
release archives are the supported paths, the way Trino ships its client as
a downloadable executable rather than through a package manager it runs.

### Install scripts

The scripts install the `engine-dev` preview by default. With a version they
install that tagged release instead and refuse to install anything whose
SHA-256 does not match the release's `SHA256SUMS` or whose `--version` does
not report the tag's version.

Windows x64, in PowerShell:

```powershell
# preview
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
# a tagged release, verified
$env:KAVEON_VERSION = "0.3.0"; irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\kaveon\bin;$env:PATH"
kaveon --version
```

A checked-out copy also takes `.\scripts\install.ps1 -Version 0.3.0`.

Linux x64 or macOS:

```bash
# preview (Linux x64 and Apple Silicon only)
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
# a tagged release, verified (adds Intel macOS)
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | KAVEON_VERSION=0.3.0 bash
```

A checked-out copy also takes `./scripts/install.sh --version 0.3.0`. Both
scripts honour `KAVEON_INSTALL_DIR` (default `~/.local/bin` or
`%LOCALAPPDATA%\kaveon\bin`) and `KAVEON_DOWNLOAD_BASE` for a mirror of the
GitHub release downloads.

### Release archives by hand

Each release has one archive per platform, each containing the binary and
`LICENSE`:

| Platform | Asset |
|---|---|
| Windows x64 | `kaveon-X.Y.Z-x86_64-pc-windows-msvc.zip` |
| macOS Apple Silicon | `kaveon-X.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `kaveon-X.Y.Z-x86_64-apple-darwin.tar.gz` |
| Linux x64 | `kaveon-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |

Download the archive and `SHA256SUMS`, verify, then extract:

```bash
V=0.3.0; T=x86_64-unknown-linux-gnu
B=https://github.com/PruthviProdduturi/Kaveon/releases/download/cli-v$V
curl -fsSLO "$B/kaveon-$V-$T.tar.gz" && curl -fsSLO "$B/SHA256SUMS"
sha256sum --ignore-missing -c SHA256SUMS      # shasum -a 256 --ignore-missing -c on macOS
tar -xzf "kaveon-$V-$T.tar.gz" kaveon && install -m 755 kaveon ~/.local/bin/kaveon
```

```powershell
$v = "0.3.0"; $b = "https://github.com/PruthviProdduturi/Kaveon/releases/download/cli-v$v"
irm "$b/kaveon-$v-x86_64-pc-windows-msvc.zip" -OutFile kaveon.zip; irm "$b/SHA256SUMS" -OutFile SHA256SUMS
(Get-FileHash kaveon.zip).Hash -eq ((Get-Content SHA256SUMS | Select-String "x86_64-pc-windows-msvc.zip") -split '\s+')[0].ToUpper()
Expand-Archive kaveon.zip -DestinationPath "$env:LOCALAPPDATA\kaveon\bin"
```

### From source

```bash
cd engine
cargo build --release -p kaveon-cli
# engine/target/release/kaveon (kaveon.exe on Windows)
```

`kaveon --help` prints the flags; `kaveon --version` prints the client version.

## Connect

The coordinator URL is positional or `--server`; a `/catalog/schema` path on
the URL selects the session context, as do `--catalog` and `--schema`:

```bash
kaveon https://engine.example.com/OpenSource/nyc_taxi
kaveon --server https://engine.example.com --catalog OpenSource --schema nyc_taxi
```

Without either, the session starts in the coordinator's default `kaveon.default`
and the status line shows no context until `USE` selects one. Options accept
`--option value` or `--option=value`. A URL may not carry credentials, a query
string or a fragment; the positional URL and `--server` cannot both be given.

Connection defaults live in `~/.kaveon_config` (or the file named by
`KAVEON_CONFIG`), one `key=value` per line, `#` comments allowed. The
allowlist is `server`, `catalog`, `schema`, `user`, `source`, `client-tags`,
`auth`, `ca-cert`, `timeout`, `output-format`, `output-format-interactive`,
`history-file`, `editing-mode`, `pager`, `theme`, `width` and `row-limit`.
SQL, access tokens and anything else are refused with the line number.
Flags on the command line override the file; a positional URL overrides the
file's `server`, `catalog` and `schema`.

`--user` is session metadata recorded with each statement; it never grants
access or replaces the authenticated identity. `--source` (default
`kaveon-cli`) and `--client-tags a,b` are recorded the same way and show up in
the Engine UI and `GET /v1/query`.

For an AKS port-forward that uses the qualification cluster's private CA:

```powershell
kubectl -n kaveon port-forward service/kaveon 18443:8080 --address 127.0.0.1
kaveon https://localhost:18443/OpenSource/nyc_taxi --ca-cert ./kaveon-ca.crt
```

The [Azure deployment guide](../engineering/azure-deployment-guide.md) covers
the certificate, Azure login and the full port-forward procedure.

## Authentication

The coordinator URL must be `https://`; plain `http://` is accepted only for
`localhost`, `127.0.0.1` and `::1`. A loopback coordinator is contacted
without the system proxy so corporate proxy settings cannot intercept it.
`--ca-cert <path>` (or `KAVEON_CA_CERT`) adds a PEM CA bundle for the
coordinator's certificate; that trust is never extended to Microsoft sign-in,
which uses the system trust store.

`--auth` selects how the client obtains a bearer token:

| Mode | Behaviour |
|---|---|
| `auto` (default) | Uses `KAVEON_ACCESS_TOKEN` or `--access-token` when set. Otherwise asks `GET /v1/auth/config`: when the coordinator advertises Microsoft Entra, takes a token from the current Azure CLI login (`az account get-access-token`, bounded to 60 s) and falls back to Microsoft device sign-in; when the coordinator has no sign-in configured (or the endpoint is absent), connects without a token. |
| `azure-cli` | Requires the Azure CLI path; fails with the `az login --tenant … --scope …` command to run when no token is available. |
| `microsoft` | Device sign-in directly: the client prints a code and a URL, then polls until the sign-in completes. Refreshes with the refresh token; on `invalid_grant` it signs in again. |
| `none` | Sends no token. The session header marks the session as insecure development. |

Tokens stay in process memory and are renewed before they expire. Prefer
`KAVEON_ACCESS_TOKEN` to `--access-token` for unattended use so the token does
not enter shell history; an empty token is an error.

## The session header

The shell prints one header into scrollback after connecting (`--no-header`
suppresses it):

```text
  ─────────────────────────────────────────────────────────────
  KAVEON  v0.3.0
  ─────────────────────────────────────────────────────────────
  Engine    http://localhost:8081  ·  v0.1.0  ·  docker
  Cluster   coordinator-1  ·  2 workers ready  ·  4.0 GiB admission
  Session   prproddu  ·  admin  ·  auth none (insecure development)

  SQL ends with ;   .help for commands   Ctrl-C cancels a running query
```

- **Engine**: the URL you connected to, the coordinator's version and its
  `environment`, both from `GET /v1/cluster`. The version is never assumed
  from the client's own.
- **Cluster**: the coordinator's node id, how many workers have a recent
  heartbeat, and the coordinator's memory admission limit. A worker whose
  heartbeat is old is counted separately (`2 workers ready · 1 stale`) in the
  warning colour; with no workers at all the line says `no workers —
  statements run on the coordinator`. When `GET /v1/cluster` fails the line
  reads `unavailable`.
- **Session**: the identity from `GET /v1/whoami` (`display` or `principal`,
  then the role `reader`, `analyst` or `admin`) and the auth mode. Older
  coordinators without `/v1/whoami` show the `--user` value instead. The note
  `(insecure development)` appears when the coordinator reports `auth:
  development` or the client ran with `--auth none`.

## The shell

The shell runs when both stdin and stdout are a terminal. It keeps a pinned
editor at the bottom of the terminal, with a status line under it, and pushes
everything that finishes (echoed statements, results, summaries, errors) into
normal scrollback above:

```text
 kaveon › SELECT region, COUNT(*) FROM kaveon_events_enriched
          GROUP BY region;
 ──────────────────────────────────────────────────────────────────────
 OpenSource.kaveon_product · localhost:8081 · 2 workers · last query 1.10 s, 18.0M rows scanned
```

Two rules decide what happens on Enter: a SQL statement ends with `;`, and a
dot command or a bare `help`, `clear`, `exit`, `quit` is one line. Everything
else is a newline.

| Key | Effect |
|---|---|
| Enter | Submits when the buffer, ignoring trailing whitespace and comments, ends with `;` (or is a one-line dot command or alias); otherwise inserts a newline. Several statements in one buffer run in order. |
| Ctrl-Enter, Alt-Enter | Submits regardless of the trailing `;`. |
| Backspace at the start of a line | Joins it to the line above. |
| Up, Down | On the first or last line of the buffer, browse history — only the entries that begin with what was typed, when something was; the unsent draft is kept and restored. Elsewhere they move the cursor. |
| Right, End, Ctrl-E at the end of the text | Take the inline suggestion: the rest of the most recent history entry that begins with what is typed, shown dimmed after the cursor. |
| Ctrl-R | Reverse search through history: type to narrow (case-insensitive, anywhere in the statement), Ctrl-R again for an older match, Backspace to widen, Enter keeps the match in the editor without running it, Esc puts the draft back. The line under the editor shows the query. |
| Ctrl-A, Ctrl-E, Ctrl-K, Ctrl-U, Ctrl-W, Alt-B, Alt-F | Readline editing: start and end of line, kill to the end and to the start of the line, kill the previous word, word back and forward. Alt-Z undoes, Alt-Y redoes. |
| Paste | Pasted text is inserted as it is, tabs as spaces, and never run: Tab inside it does not complete and a newline inside it does not submit. Enter after it runs the statement. |
| Tab | Completes the word before the cursor: SQL keywords, dot commands, and catalog, schema, table and column names fetched lazily from the catalog API (the cache is refreshed by `USE`). One candidate is inserted; several are listed above the editor and the common prefix is inserted. |
| Ctrl-C | Cancels the running statement (see below); with nothing running, clears the buffer; with an empty buffer, prints how to leave. |
| Ctrl-D, `exit`, `quit`, `.quit` | Leaves and saves history. |
| Ctrl-L, `clear`, `.clear` | Clears the screen. |

The editor grows with the statement up to six lines, then scrolls inside.
SQL keywords are highlighted in the accent colour, strings in green and
comments dimmed; the text is never altered. The status line shows the
session `catalog.schema` (once one is selected), the coordinator host, the
worker count, and the last statement's duration and scanned rows; it turns
to the warning colour when the cluster poll (every 30 s) finds a stale
worker.

`--editing-mode vi` is accepted; today it applies to the line-editor
fallback the client uses when stdout is not a terminal, not to the shell's
editor, which uses Emacs-style keys. `--output-format-interactive` selects
the result format for the shell independently of `--output-format` used by
scripts.

## Running a statement

The submitted statement is echoed dimmed after the `kaveon ›` prompt, the
editor dims and shows what is running, and a running line appears above it:

```text
 ⠸ Running 2.4 s · 3/5 tasks · 2 workers · 210M rows scanned          Ctrl-C to cancel
```

The statement is posted on a worker thread with a unique
`kaveon-cli:<uuid>` client tag. The shell finds the query record by that tag
in `GET /v1/query` and then polls `GET /v1/query/{id}` every 250 ms. The line
reads `Submitting …` until the record is found, `Queued … for memory
admission · N ahead` while the coordinator holds the statement for memory
(the queue depth comes from `GET /v1/cluster`), then `Running …`. Task
counts, worker count, rows scanned and `waited N ms for admission` are shown
only when the record reports them; a coordinator that publishes counters
only at completion shows elapsed time and state until then.

Rows appear while the statement is still running. The shell asks for paged
delivery, and the coordinator writes the result a page at a time — 1,000
rows or 4 MiB, whichever comes first. Once the query record carries the
result's `next_uri`, the shell asks for page 0 every 500 ms
(`GET /v1/query/{id}/results/0`, bounded at 5 s per request); until the
page is written the coordinator answers `202 Accepted` with the rows it has
so far, and the running line says `· 12,000 rows so far`. When the page
arrives it is rendered under the running line, above the editor, with its
header, and paging takes over: the hint reads `1,000 rows so far · Space or
Enter for more · q to stop` (`1,000 of 84,312 rows` once the writer is
complete), the running line stays above it and Ctrl-C still cancels. Space
or Enter for a page the coordinator has not written yet shows `waiting for
the next page…` and asks again every 500 ms until it arrives or `q` stops.
When the statement finishes, page 0 is not rendered again; the summary
follows the paging's closing line (`all 84,312 rows shown`, `stopped after
2,000 rows`), or comes at once if the reader already stopped. A failure or
cancel while the pages are being read ends the paging with the error or
cancel summary.

What arrives early depends on the plan. A scan, a filter, a projection and
a `LIMIT` emit rows as each input batch is processed, so the first page
shows within moments of the first 1,000 rows. `ORDER BY`, `GROUP BY` and
`DISTINCT` emit nothing until the last input batch has been consumed, so
their pages are written only at the end and the running line's `rows so
far` stays at 0 until then. This holds on a cluster as it does on one
node: a worker streams a root task's rows to the coordinator while the
task runs, batch by batch, and the coordinator pages them as they arrive —
a scan split over two workers shows its first page while both are still
scanning. A result within the row limit (the default `.limit 1000`) is a
single page, written when the statement completes. Scripts (`-e`, `-f`,
piped input) and inline delivery are unchanged: the rows come when the
statement finishes.

**Ctrl-C** while a statement runs sends `DELETE /v1/query/{id}`; the line
reads `Cancelling …` until the coordinator lets go, and the summary then
says `✗ cancelled after 4.10 s`. A second Ctrl-C, or a first one before the
record has an id, abandons the wait: the editor returns at once and the
statement may still finish on the coordinator (`.queries` and `.kill <id>`
reach it). A statement cancelled from elsewhere (the Studio, another client)
is reported the same way.

`--timeout` (default **24 h**; `30s`, `2m` and bare seconds are accepted)
bounds each statement request. `POST /v1/statement` returns only when the
statement finishes, so this is the longest one statement may take before the
client reports `Connection: coordinator request timed out`; it does not stop
the statement on the coordinator. Metadata calls (`/v1/cluster`, `/v1/query`,
the catalog API) keep a fixed 30 s bound.

## Analyzing a run

```text
kaveon OpenSource.kaveon_product › EXPLAIN ANALYZE SELECT region, count(*) AS users
                                   FROM kaveon_events_enriched WHERE country = 'India'
                                   GROUP BY region ORDER BY users DESC;
  Sort order_by=[(Column("users"), false)]
  └─ Project expressions=[Column("region"), Alias { … name: "users" }]
     └─ Aggregate aggregates=[Count { expr: Star, distinct: false }] group_by=[Column("region")]
        └─ Filter predicate=BinaryOp { left: Column("country"), op: Eq, right: Literal(Utf8("India")) }
           └─ Scan columns=country, region table=OpenSource.kaveon_product.kaveon_events_enriched

  Execution  distributed · fragments   analysis 86 µs · planning 52 µs · execution 15.93 s

  Stage 0  FINISHED · 2/2 tasks · 15.93 s · 2 nodes
  ┌──────┬──────────┬─────────┬────────┬─────────────┬──────────────┬───────────────┬──────────┬─────────────┬──────────────┬─────────┐
  │ task │ node     │ elapsed │    cpu │ peak memory │ rows scanned │ bytes scanned │ rows out │ exchange in │ exchange out │ spilled │
  ├──────┼──────────┼─────────┼────────┼─────────────┼──────────────┼───────────────┼──────────┼─────────────┼──────────────┼─────────┤
  │ 0.0  │ worker-1 │ 15.82 s │ 4.45 s │   929.8 KiB │   16,300,620 │     234.1 MiB │        0 │         0 B │      4.5 KiB │ —       │
  │ 0.1  │ worker-2 │ 15.84 s │ 4.44 s │   929.8 KiB │   16,300,620 │     234.1 MiB │        0 │         0 B │      4.5 KiB │ —       │
  └──────┴──────────┴─────────┴────────┴─────────────┴──────────────┴───────────────┴──────────┴─────────────┴──────────────┴─────────┘

  Stage 1  FINISHED · 2/2 tasks · 92 ms · 2 nodes
  …
 ✓ 15.93 s · 1 row · 2 workers · 32.6M rows scanned at 2.0M rows/s · 468.2 MiB read   19163173
```

`EXPLAIN ANALYZE` runs the statement (result cache off, rows discarded) and
reads its query record: the optimized plan, the execution mode
(`distributed · fragments`, or the coordinator-local modes), the phase
timings, then each stage with its state, tasks done, elapsed time and node
count, and a table with one row per task. The columns are the record's task
counters: `elapsed` and `cpu` (compute CPU time), `peak memory`, `rows
scanned` and `bytes scanned` (the task's Parquet reader: rows emitted and
compressed bytes selected), `rows out` and the exchange bytes it received and
sent, `spilled` (bytes written to disk, `—` when none). A leaf stage's tasks
send their partial results through the exchange, so their `rows out` is 0 and
`exchange out` carries the volume. A statement answered on the coordinator has
no stages and says so. Nothing is estimated: a counter the coordinator did not
record shows as `—`.

## Results

In the shell, `ALIGNED`, `AUTO` and the legacy `table` format render a styled
table: light box-drawing borders, the header in the accent colour, integers
right-aligned with thousands separators, floats with four decimals, `NULL`
dimmed, strings left-aligned. String columns wider than the terminal
(`--width` overrides the detected width) are narrowed with `…`, never below
eight characters, and the summary says so; `.format VERTICAL` shows the
rows whole (format names are the uppercase ones listed under Output
formats, plus the legacy lowercase four). `AUTO` switches to the vertical
format when even the narrowed table does not fit. Other formats print
exactly what scripts get.

Under every result, unless `.timing` has turned it off:

```text
 ✓ 1.10 s · 5 rows · 2 workers · 18.0M rows scanned at 16.4M rows/s · 6.2 MiB read   66aea874
   waited 12 ms for admission
```

The first line is the verdict: elapsed time, rows returned, the distinct
worker nodes that ran tasks, scanned rows with the rate over the whole
elapsed time, compressed bytes read, and the query id (first eight
characters). Scan counters come from the query record; when the workers'
Parquet coverage is incomplete the line says `partial worker metrics`
instead of guessing. A second, dimmed line appears only when there is
something exceptional to say: `from cache`, `on the coordinator: <reason>`,
`N/M tasks` when not every task completed, `waited N ms for admission`, the
row-limit note, or the truncation note. Metadata commands end with
` ✓ 12 ms · 2 catalogs · catalog API` so a catalog answer is never mistaken
for a statement.

**Row limit.** Queries without a top-level `LIMIT` show the first 1,000
rows: the shell appends `LIMIT 1000` before sending and the summary says
`showing the first 1,000 rows of a query without LIMIT`. `.limit <n>` or
`.limit off` changes that for the session, and `--row-limit <n>` (or
`row-limit=` in the defaults file) sets it at startup. Results longer than a
page are shown a page at a time, each page as soon as the coordinator has
written it (see Running a statement): Space or Enter for the next 1,000
rows, `q` to stop. The summary counts the whole result once a page has said
the writer is complete; a reader who stops earlier sees the rows shown.
Scripts (`-e`, `-f`, piped input) are never limited; a script whose
result would exceed the coordinator's 16 MiB inline ceiling should run with
`--paged`, which fetches result pages instead of failing.

## Errors

A failed statement is one panel in scrollback: the kind and the query id,
the message once, and the statement's first lines with a caret where the
coordinator pointed:

```text
 ✗ SQL parse error   query 7c0c9b9e
   Expected an expression, found: FROM
   SELECT FROM t
          ^
```

Kinds are `SQL parse error`, `Planning error`, `Worker failure`, `Memory
admission`, `Cancelled`, `Connection`, `Authentication`, `Not found` and
`Coordinator` (anything else), mapped from the HTTP status and the response's
`code`. Worker failures that repeat the same message on several workers
collapse to one line followed by the set of workers, and JSON error bodies
are unwrapped; transport prefixes (`coordinator returned HTTP …`,
`execution: …`) are stripped. The caret comes from the `position` the
coordinator sends with parse errors (line and column, one-based) or from a
trailing `at line N, column M` in the message; without one, the excerpt is
shown without a caret. Column and table-not-found errors from the workers
have no position today.

The client resolves a few errors before showing them:

- `SHOW CATALOGE` → `unsupported SHOW CATALOGE; did you mean SHOW CATALOGS?`;
  singular forms (`SHOW CATALOG`, `SHOW SCHEMA`, `SHOW TABLE`) are accepted.
- `.tabels` → `unknown command '.tabels'; did you mean .tables?`; a
  `.settings` key or `USE` target with a typo is corrected the same way, and
  `USE` lists the catalogs or schemas that exist when nothing is close.
- A missing table is looked up across the catalogs: when it exists in exactly
  one `catalog.schema` the message says so and how to get there (`run USE
  lake.gold; or query lake.gold.orders`); when it exists in several, they
  are listed; otherwise the closest table name is suggested.

## Commands

Metadata statements are answered by the client over the catalog API without
a coordinator statement; everything else is SQL for `POST /v1/statement`.

| Command | Effect |
|---|---|
| `SHOW CATALOGS [LIKE 'p']` | Catalogs; `SHOW CATALOG` is accepted |
| `SHOW SCHEMAS [IN catalog] [LIKE 'p']` | Schemas of the session catalog or a named one; `FROM` equals `IN` |
| `SHOW TABLES [IN [catalog.]schema] [LIKE 'p']` | Tables; `LIKE` filters names with `%` and `_` in the client |
| `DESCRIBE [catalog.][schema.]table`, `DESC`, `SHOW COLUMNS FROM …` | Column name, type and nullability from the catalog definitions |
| `ANALYZE [catalog.][schema.]table [WITH (…)]` | Coordinator statement (admin role): collects the table's row count, file count and bytes, and each column's null fraction, minimum, maximum and data size from metadata alone. `WITH (distinct = true)` also counts the distinct values of every column, `WITH (columns = ARRAY['a', 'b'])` of the columns named — exact, one `COUNT(DISTINCT)` statement per column through the cluster, cancellable with the `ANALYZE`; a column not counted keeps its previous count while the table's files are unchanged. The result adds `distinct_columns`, how many were counted. |
| `SHOW STATS FOR [catalog.][schema.]table` | Coordinator statement: the column statistics of the last `ANALYZE`, one row per column plus a summary row with the table's row count. The shell shows a header line (`catalog.schema.table · 3,000,000 rows · 40.2 MiB`) over the columns with nulls as a percentage and sizes humanised; `SHOW STAT FOR` is accepted. A table never analyzed answers `Not found: no statistics for …; run ANALYZE …`. |
| `DESCRIBE DETAIL [catalog.][schema.]table` | Coordinator statement: format, location, created and modified times, file count, size, row count, Delta version, partition columns, when it was analyzed and the catalog snapshot, shown in the shell as a `field \| value` list |
| `OPTIMIZE [catalog.][schema.]table [WITH (…)] [WHERE …]` | Coordinator statement (admin role): rewrites a Parquet table's files in the layout its definition declares — sorted by its `clustered_by` columns, 128 MiB / 1 M-row row groups with a page index and Bloom filters (`WITH (row_group_rows = …, row_group_bytes = …, file_bytes = …)` overrides the sizes); `WHERE` selects the files to rewrite by partition values and footer statistics. One row: files replaced and written, rows, row groups, bytes before and after, the clustering, and how many interrupted rewrites were recovered. Delta and Iceberg tables are refused: their files are named by a log the Engine does not write. See [Layout](../engine/storage-and-catalogs.md#layout) |
| `ALTER TABLE [catalog.][schema.]table SET CLUSTERED BY (a, b)` | Coordinator statement (analyst or admin role): records the clustering the next `OPTIMIZE` writes; `()` clears it |
| `USE [catalog.]schema`, `USE catalog` | Validates the target before switching. A bare name is the schema in the current catalog when it exists, else the catalog of that name (keeping the current schema when it has it, or its only schema). |
| `EXPLAIN <statement>` | Runs the statement with the result cache off, discards the rows and prints the logical plan as an indented tree, then the summary |
| `EXPLAIN ANALYZE <statement>` | The same over the optimized plan (pruned columns, pushed filters), followed by what the run cost: the coordinator's analysis, planning and execution times and, per stage, one row per task — node, elapsed, CPU, peak memory, rows and bytes scanned, rows out, exchange bytes in and out, bytes spilled — exactly as the query record reports them. See [Analyzing a run](#analyzing-a-run) |
| `SET SESSION key = value; <statement>` | Passed through to the coordinator unchanged |
| `.catalogs`, `.schemas [catalog]`, `.tables [[catalog.]schema]`, `.describe <table>`, `.use <target>` | The metadata statements without a semicolon |
| `.limit [n]` | Show or set the interactive row limit |
| `.format <name>` | Change the result format for the session (`ALIGNED`, `VERTICAL`, `AUTO`, `MARKDOWN`, `CSV`, `TSV`, `JSON`, `NULL`, or the legacy lowercase names) |
| `.timing` | Toggle the summary lines |
| `.history [n]` | The last *n* statements of this session (default 20) with duration and outcome |
| `.cluster` | One line per node (role, version, heartbeat age, RSS, limit) and the coordinator's admission state and queue depth |
| `.queries` | Statements queued or running on the coordinator now, with id, state, elapsed time and tags |
| `.kill <id>` | `DELETE /v1/query/{id}` for a statement of any client |
| `.settings`, `.settings <key> <value>`, `.settings reset` | Show, set or clear the session settings (below) |
| `.ask <question>`, `.ask <n>` | A question in plain language, answered through the Kaveon DLM on the platform API (`--api`); `<n>` answers a clarification. See [Asking in plain language](#asking-in-plain-language) |
| `.source <file>` | Runs the statements of a file through the shell in order, each with its result and summary; a path with spaces is quoted |
| `.tee <file>`, `.tee off` | Appends everything shown from then on — echoed statements, results, summaries, errors, without colour — to the file; the status line names it |
| `.edit` | Opens the last statement in `$VISUAL`, else `$EDITOR` (else `notepad` or `vi`); on return the text is loaded into the editor, Enter runs it |
| `<statement>\G` | Ends a statement like `;` and shows that result in the vertical format once |
| `.watch [seconds] <statement>` | Re-runs the statement every *seconds* (default 2, at most 86,400), clearing the screen each run, until any key |
| `.help`, `.h`, `help` | The command reference, grouped, one screen |
| `.clear`, `clear` | Clear the screen |
| `.quit`, `.exit`, `.q`, `exit`, `quit` | Leave |

`.settings` keeps a `settings` object that is sent with every statement. The
keys and the server settings they set (see
[per-request settings](../engine/settings.md#per-request-settings)):

| Key | Value | Server setting |
|---|---|---|
| `memory` | bytes or a size such as `256MiB`, `1GiB` | `query_memory_limit_bytes` |
| `parallelism` | 1 to 1024 | `local_parallelism` |
| `cache` | `on` or `off` | `result_cache` |
| `admission_wait` | seconds, 0 to 86400 | `admission_wait_seconds` |

Values are validated in the client against the same ranges the coordinator
enforces; the coordinator still caps them at its own limits and answers HTTP
400 `INVALID_SETTING` for anything it refuses. The Engine's HTTP API is
stateless, so a setting lives only as long as the statements that carry it.

## Asking in plain language

```text
kaveon OpenSource.kaveon_product › .ask users by platform in Europe
→ Product users · Users by platform, Europe
SELECT platform, COUNT(*) AS users
FROM kaveon_events_users
WHERE region = 'Europe'
GROUP BY platform
ORDER BY users DESC
confidence 0.91
┌──────────┬─────────┐
│ platform │   users │
…
```

`.ask` sends the question to the Kaveon DLM — the platform's deterministic
data language model, not a hosted LLM — at `POST /api/v1/dlm/ask` on the
platform API named by `--api <url>` or `KAVEON_API_URL` (`KAVEON_API_TOKEN`
is sent as a bearer token when set). The DLM routes the question to a
registered dataset and answers in one of four shapes:

- **Live**: the SQL it wrote, highlighted, with the dataset, title, note and
  confidence. When the dataset is a native Kaveon catalog the shell switches
  the session to that catalog and schema and runs the SQL on the coordinator
  as any statement, with progress, the result table and the summary. When
  the dataset is served by the platform (a PostgreSQL or Fabric source) the
  SQL is shown to run in SQL Lab.
- **From context**: the rows answered from the DLM's precomputed context
  without a scan, marked `from context · no scan` (`≈ approximate` for a
  sketch-backed distinct count).
- **Clarify**: a numbered list of the readings the question could have;
  `.ask <n>` chooses one and the answer follows. A word that looks like a
  filter but is no value the DLM knows is never dropped silently: a near
  miss (`desktop users in finance`) is a clarification listing the closest
  values (`industry = Financial Services`) and the choice to leave the word
  out; a word with nothing close is left out and the answer's note says so,
  as does a `by` phrase naming a column that is not one of the dataset's
  dimensions. A later question inherits the previous answer's frame, so
  `.ask and in Asia` narrows the same question.
- **Out of scope** or refused: one line saying why, with the registered
  datasets when there are any.

In scripts (`-e ".ask …"`) a question is answered once: a Live answer over a
native catalog runs and prints its result; a clarification lists its choices
and asks for the choice to be put in the question, since a script keeps no
session. Without `--api` the command explains where the DLM runs.

## Catalog administration

`kaveon catalog|schema|table …` registers catalogs, schemas and tables from
the command line, the way `trino --execute "CREATE TABLE …"` or the
`register_table` procedure would. Each command is one catalog statement
submitted to the coordinator through `POST /v1/statement` with the same
connection options as the shell (`--server`, `--catalog`, `--schema`,
`--auth`, `--access-token` or `KAVEON_ACCESS_TOKEN`, `--ca-cert`,
`--timeout`, `--output-format`), so the role checks, revisions and audit
trail are the coordinator's. Unqualified names resolve against the session
`--catalog` and `--schema`.

```bash
# Catalogs (admin role)
kaveon catalog list [--like 'pattern']
kaveon catalog show Benchmarks                      # the durable definition as JSON
kaveon catalog add Benchmarks --storage adls --account kvtest --container opensource \
    --root benchmarks --credential workload-identity:kaveon-test-reader
kaveon catalog add local --storage local --base-path /data/warehouse
kaveon catalog drop staging --cascade

# Schemas (analyst or admin role)
kaveon schema list [catalog]
kaveon schema add Benchmarks.tpch_sf100 --if-not-exists
kaveon schema drop Benchmarks.tpch_sf100 --cascade

# Tables (analyst or admin role)
kaveon table list Benchmarks.tpch_sf100 [--like 'pattern']
kaveon table register Benchmarks.tpch_sf100.lineitem --location tpch/sf100/lineitem --format delta
kaveon table register Benchmarks.clickbench.hits --location clickbench/hits.parquet --format parquet \
    --columns 'WatchID bigint, JavaEnable smallint, Title varchar'
kaveon -e "ALTER TABLE Benchmarks.clickbench.hits SET CLUSTERED BY (EventDate, CounterID)"
kaveon -e "OPTIMIZE Benchmarks.clickbench.hits"       # admin: rewrite the files in that layout
kaveon table relocate Benchmarks.tpch_sf100.lineitem --location tpch/sf100-v2/lineitem
kaveon table describe Benchmarks.tpch_sf100.lineitem
kaveon table show-create Benchmarks.tpch_sf100.lineitem
kaveon table stats Benchmarks.tpch_sf100.lineitem       # SHOW STATS FOR: the last ANALYZE
kaveon table detail Benchmarks.tpch_sf100.lineitem      # DESCRIBE DETAIL: format, location, files, size, versions
kaveon table drop Benchmarks.tpch_sf100.lineitem --if-exists
```

`table stats` and `table detail` submit `SHOW STATS FOR` and `DESCRIBE
DETAIL` and print the rows as the coordinator returns them in the chosen
`--output-format` (the humanised header and percentages are the shell's).
Statistics exist once an admin has run `ANALYZE table` in the shell or with
`-e`; until then `table stats` answers `no statistics for …; run ANALYZE …`.

`table register` without `--columns` has the coordinator read the columns
from the table itself (the Delta log, the Iceberg metadata pointer, or the
Parquet footers) and store them; with `--columns`, the declared list is
stored once every column is found in the source. Either way the location is
probed with a metadata-only read before the table is activated: an
unreadable location — a missing object, a Parquet file registered as Delta,
a column the source does not have — fails the command with the storage
error and registers nothing. `--location` is a path within the catalog's
storage root (a container-relative path for ADLS, a directory under
`base_path` for a local catalog), not a URI. A mistaken option is reported
by the client, naming the option, before anything reaches the coordinator.

The layout of a table — `clustered_by` and `bloom` — is part of its
definition (`CREATE TABLE … WITH (…, clustered_by = ARRAY['a'], bloom =
ARRAY['b'])`, `ALTER TABLE … SET CLUSTERED BY (…)`), and `OPTIMIZE` writes
it; the CLI has no data-writing command of its own, so there is no
`--cluster-by` flag: the statements run in the shell or with `-e` as above.

The same statements run in the shell and with `-e`, so a script of
`CREATE SCHEMA` / `CREATE TABLE … WITH (…)` statements registers a catalog's
tables in one `kaveon -f register.sql`; the full grammar, including
`CALL system.register_table(schema_name => …, table_name => …,
table_location => …)`, is in the
[API reference](../reference/api.md#catalog-statements). Every command
returns one row — the object and `created`, `exists`, `dropped`, `absent`,
`relocated` or `unchanged` — and leaves a query record on the coordinator.

## Scripts, history, and output

## Output formats

`--output-format` (scripts) and `--output-format-interactive` / `.format`
(shell) accept the legacy lowercase names `table`, `csv`, `tsv` and `json`,
which keep their existing serialisation, and the uppercase Trino-style names:

```text
ALIGNED  VERTICAL  AUTO  MARKDOWN
CSV  CSV_HEADER  CSV_UNQUOTED  CSV_HEADER_UNQUOTED
TSV  TSV_HEADER  JSON  NULL
```

`JSON` emits one JSON object per row; `NULL` discards rows (the summary is
not printed for machine formats); `AUTO` honours `COLUMNS` and otherwise
assumes 120 columns before choosing aligned or vertical output. The names
cover the everyday Trino CLI workflows and are not a claim of byte-for-byte
formatter parity.

## Scripts

```bash
kaveon https://engine.example.com/lake/gold -e "SELECT COUNT(*) FROM orders;" --output-format JSON
kaveon https://engine.example.com -f nightly.sql --ignore-errors
cat nightly.sql | kaveon https://engine.example.com --output-format CSV_HEADER
```

`-e` runs the statements in its argument, `-f` a UTF-8 file, and piped stdin
is read whole; statements run in order and stop at the first failure. With
`--ignore-errors` the rest still run and the exit code stays nonzero. Exit
codes: `0` success, `1` a failed statement or connection, `2` a usage error.
Dot commands work in scripts (`.use lake.gold` then a statement). Each
failure is one line on stderr:

```text
error: <kind>: <message> (query <id>)
```

with the same kinds as the panel and the query id when the coordinator
assigned one. Nothing is decorated: no header, no colour, no summary for
machine formats, and the interactive row limit is not applied. Results are
requested inline, which the coordinator caps at 16 MiB; pass `--paged` for a
script whose result may exceed that. A paged result spools on the
coordinator under `KAVEON_RESULT_QUERY_DISK_LIMIT_BYTES` (256 MiB unless the
operator raised it; the failure names the setting).

## History

The shell keeps history at `%APPDATA%\kaveon\history` on Windows and
`~/.kaveon_history` elsewhere; `--history-file <path>` (or
`KAVEON_HISTORY_FILE`) moves it and `--no-history` disables it. Multi-line
statements are stored whole, one entry per line with newlines escaped, and
come back intact on Up. The last 1,000 entries are written on exit;
consecutive duplicates and `exit` are not recorded. `.history [n]` shows the
statements of the current session with their duration and outcome, which is
separate from the file.

## Appearance

`--theme dark` (default), `light` (a darker Kaveon blue for light
backgrounds) or `mono` (no colour). `NO_COLOR` set to anything, `TERM=dumb`,
or output that is not a terminal also give plain text. The accent is Kaveon
blue (`#4A9EE8`, the Studio accent; `#2D7DD2` on light); everything else is
dim grey, with yellow for warnings, red for errors and green for the
summary's check mark. `--no-header` skips the session header; `--width
<cols>` fixes the width tables are fitted to instead of detecting it.
`--pager <program>` (or `KAVEON_PAGER`) pipes human-readable output through
an external pager when a script or the line-editor fallback prints to a
terminal; an empty value disables it. The shell does its own paging.

## Embedded mode

`--local` runs the Engine in the client's process over local Parquet and
Delta tables, with no coordinator. It uses the line-based REPL and the plain
renderers, not the shell above; statements do not appear in any Engine query
history.

```bash
kaveon --local --data-dir /path/to/parquet     # one catalog discovered from a directory
kaveon --local                                 # ~/.kaveon/config.toml and ~/.kaveon/catalogs/*.toml
kaveon --local --config /path/to/config.toml
```

`--data-dir` (`-d`) registers each immediate lowercase `*.parquet` file as a
table in the `default` schema, named after the file, and each immediate
child directory with a `_delta_log` as a Delta table named after the
directory; it does not recurse further. Delta reads replay the JSON commits
and v1 checkpoints (classic or multipart) to one pinned version; an
incomplete history is refused rather than read partially, and reader
protocol v2 features (column mapping, deletion vectors) and v2 checkpoint
sidecars are refused by name.

For several catalogs, use the Trino-like layout:

```text
~/.kaveon/
├── config.toml          default_catalog = "sales"   default_schema = "default"
└── catalogs/
    ├── sales.toml       type = "local"   base_path = "/data/sales"
    └── operations.toml
```

A catalog with no `[[table]]` entries discovers `base_path` the way
`--data-dir` does. Explicit tables name their schema, location (relative to
`base_path`; a file for Parquet, a directory for Delta) and format:

```toml
type = "local"
base_path = "/data/warehouse"

[[table]]
name = "orders"
schema = "commerce"
location = "orders.parquet"
access = "shortcut"
format = "parquet"

[[table]]
name = "order_events"
schema = "commerce"
location = "order_events"
access = "shortcut"
format = "delta"
```

When a `catalogs` directory exists next to the configuration file its
`.toml` and `.properties` files are separate catalogs; otherwise the older
single-file `[[catalog]]` layout in `engine/kaveon.example.toml` is read.
`SHOW CATALOGS`, `SHOW SCHEMAS [FROM catalog]`, `SHOW TABLES [FROM
catalog.schema]`, `DESCRIBE`, `USE` and the dot commands work in this mode.
The embedded planner does not run the binder, so comma joins are cross
products; Iceberg, ADLS Gen2 and S3 locations, and `ANALYZE`, need the
server. See [Engine SQL compatibility](../reference/engine-sql-compatibility.md)
for the SQL surface.

## Not yet

Everything `.help` lists is implemented. What the client does not do yet:
per-operator statistics inside a plan (`EXPLAIN ANALYZE` is per task);
spooling to object storage; Kerberos, JWT and HTTP-proxy options;
package-manager listings — a later item with no timeline: the winget
manifest and Homebrew formula are rendered with every release, the
submissions are described in [Cutting a CLI release](../engineering/cli-release.md).
Rows appear while a
statement runs only where the plan's root emits them early: a scan, a
filter, a projection, a `LIMIT` without `ORDER BY` and a join's probe
output stream from their first batch, on one node and across workers
alike; a root that is an `ORDER BY`, `GROUP BY` or `DISTINCT` holds
everything until its last input batch, so its rows still show at the end —
see [Running a statement](#running-a-statement).

## Troubleshooting

**`Connection: coordinator request timed out` vs `✗ cancelled after …`.**
The first is the client giving up on the HTTP request after `--timeout`
(24 h by default, so it usually means a much shorter value was set); the
statement may still be running, and `.queries` shows it. The second is a
cancel the client sent (Ctrl-C, `.kill`) or one made elsewhere; the
coordinator has stopped the statement.

**`0 workers · 2 stale` in the status line under load.** The coordinator
drops a worker after 30 s without a heartbeat and the client marks one stale
before that when its heartbeat age is large. Workers busy with a heavy
statement can heartbeat late; the line recovers on the next 30 s poll.
`.cluster` shows each node's heartbeat age and RSS.

**HTTP 507 (insufficient storage).** The coordinator's exchange or result
spool is full. The Engine's defaults are 10 GiB per node and 8 GiB per query;
`docker-compose.yml` raises them to 24 GiB and 16 GiB, which a repartitioned
exact `COUNT(DISTINCT …)` over the largest demo table needs. See the
[local stack guide](local-stack.md#resource-notes).

**`inline results exceed 16 MiB; request result_delivery=paged` from a
script.** Inline results are capped at 16 MiB.
Add a `LIMIT`, choose a narrower projection, or run the script with
`--paged`.

**`Engine URL requires HTTPS`.** Plain HTTP is only accepted for loopback
hosts. Port-forward the coordinator to `localhost` or give it a certificate
and pass its CA with `--ca-cert`.

**Azure CLI token failures.** The message names the `az login --tenant …
--scope …` command; `--auth microsoft` uses device sign-in instead.
