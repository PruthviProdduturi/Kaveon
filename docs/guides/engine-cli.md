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

Windows x64, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\kaveon\bin;$env:PATH"
kaveon --version
```

Linux x64 or Apple Silicon macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
```

Both scripts download the `engine-dev` preview build published by the
[Engine workflow](../../.github/workflows/engine.yml). From source:

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
| Up, Down | On the first or last line of the buffer, browse history; the unsent draft is kept and restored. Elsewhere they move the cursor. |
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
page are shown a page at a time: Space or Enter for the next 1,000 rows, `q`
to stop. Scripts (`-e`, `-f`, piped input) are never limited; a script whose
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
| `USE [catalog.]schema`, `USE catalog` | Validates the target before switching. A bare name is the schema in the current catalog when it exists, else the catalog of that name (keeping the current schema when it has it, or its only schema). |
| `EXPLAIN <statement>` | Runs the statement with the result cache off, discards the rows and prints `plan.logical` as an indented tree with the summary |
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
kaveon table relocate Benchmarks.tpch_sf100.lineitem --location tpch/sf100-v2/lineitem
kaveon table describe Benchmarks.tpch_sf100.lineitem
kaveon table show-create Benchmarks.tpch_sf100.lineitem
kaveon table drop Benchmarks.tpch_sf100.lineitem --if-exists
```

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
script whose result may exceed that.

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

## Coming soon

Listed in `.help` and rendered dimmed until each one ships:

- `.ask <question>` — answer a question in plain language through the Kaveon DLM.
- Paged results — large results page on demand instead of the 10,000-row ceiling.
- `.edit`, `.source <file>`, `.tee <file>` — edit in `$EDITOR`, run a script, copy output to a file.
- `\G` and `.watch <seconds>` — vertical output for one statement; re-run on an interval.

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
