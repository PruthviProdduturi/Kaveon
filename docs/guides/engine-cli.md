# Kaveon Engine CLI

Status: **Alpha**. Kaveon CLI is an interactive client for a Kaveon Engine
coordinator. Remote mode is the default; `--local` runs the embedded Engine for
local Parquet and Delta work.

## Install the remote client

Windows x64 does not require a source clone or Rust toolchain. In PowerShell,
install the current `engine-dev` preview client:

```powershell
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\kaveon\bin;$env:PATH"
kaveon --version
```

On Linux x64 or Apple Silicon macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
```

For a complete AKS connection walkthrough, including the public CA certificate,
see [Kaveon on Azure: deploy, connect and test](../engineering/azure-deployment-guide.md).

## Connect to a coordinator

Remote mode submits SQL to the coordinator and is the default:

```bash
kaveon --server https://engine.example.com --catalog OpenSource --schema nyc_taxi
# The URL may be positional; its /catalog/schema path selects session context.
kaveon https://engine.example.com/OpenSource/nyc_taxi
```

Options accept either `--option value` or `--option=value`. Use
`--client-request-timeout 30s` or `--client-request-timeout 2m` to set a request
timeout. `--access-token` supplies an explicit bearer token; prefer the
`KAVEON_ACCESS_TOKEN` environment variable for unattended use so the token does
not enter shell history.

When the coordinator enables Microsoft sign-in, `--auth auto` (the default)
first reuses the current Azure CLI login and renews through it when necessary;
it falls back to Microsoft device sign-in. Use `--auth azure-cli` to require that
Azure CLI path, or `--auth microsoft` to use device sign-in directly. The client
keeps tokens only in process memory. `--user` is session metadata and never
grants access or changes ownership.

For an AKS port-forward that uses the test private CA:

```powershell
kubectl -n kaveon port-forward service/kaveon 18443:8080 --address 127.0.0.1
kaveon --server https://localhost:18443 --ca-cert ./kaveon-ca.crt --catalog OpenSource --schema nyc_taxi
```

Use the [Azure deployment guide](../engineering/azure-deployment-guide.md) for
certificate handling, Azure login, and the full port-forward procedure.

In the remote shell, the prompt displays the selected schema. `help`, `clear`,
`exit`, and `quit` accept an optional trailing semicolon; `.help`/`.h`,
`.clear`, and `.quit`/`.exit`/`.q` are equivalent shortcuts. Human-readable
output includes blank-line separation and aligned numeric metadata values.

## Remote catalog navigation

The following commands work in the remote CLI through authenticated catalog GET
APIs. They are client metadata commands, not SQL support provided by
`POST /v1/statement`:

```sql
SHOW CATALOGS;
SHOW SCHEMAS IN OpenSource;
SHOW TABLES FROM OpenSource.nyc_taxi;
SHOW TABLES FROM OpenSource.nyc_taxi LIKE '%_trips';
DESCRIBE OpenSource.nyc_taxi.yellow_trips;
SHOW COLUMNS FROM OpenSource.nyc_taxi.daily_trips;
USE OpenSource.nyc_taxi;
```

`SHOW SCHEMAS` and `SHOW TABLES` accept `IN` or `FROM`; unqualified commands use
the validated session catalog and schema. Dot aliases are `.catalogs`,
`.schemas [catalog]`, `.tables [[catalog.]schema]`, `.describe <table>`, and `.use <catalog.schema>`.
`LIKE` filters returned names with SQL `%` and `_` wildcards. `DESCRIBE` and
`SHOW COLUMNS` read catalog-definition metadata for recorded name, type, and
nullability.
`USE` validates its target before changing the prompt context.

Connection defaults can be stored in `KAVEON_CONFIG`, or in
`~/.kaveon_config` when that variable is unset. It is a `key=value` file whose
allowlist is limited to connection, output, history, and pager defaults; it
cannot contain SQL or access tokens. Explicit CLI flags override these defaults.

After a completed remote query, the CLI can show the returned row count and JSON
result bytes with rates, plus reported node/task counts. Complete Parquet worker
coverage reports measured reader-output rows, selected row-group rows, and
selected compressed bytes; incomplete coverage is unavailable rather than zero.

## Scripts, history, and output

Use `-e` for a statement or `-f`/`--file` for a UTF-8 script. Statements in a
script run in order; `--ignore-errors` continues after failures while preserving
a nonzero exit status. Interactive history is persistent by default at
`%APPDATA%\kaveon\history` on Windows (or `~/.kaveon_history` elsewhere); set a
different location with `--history-file` or disable it with `--no-history`. `--editing-mode`
accepts `EMACS` (default) or `VI`; SQL keyword completion is available in the
interactive terminal, and `--disable-auto-suggestion` turns off history
suggestions. `--pager` selects an optional pager and an empty value disables it.

Lowercase `table`, `csv`, `tsv`, and `json` preserve the existing Kaveon output
formats. The CLI also accepts exact uppercase Trino-style names:

```text
ALIGNED  VERTICAL  AUTO  MARKDOWN
CSV  CSV_HEADER  CSV_UNQUOTED  CSV_HEADER_UNQUOTED
TSV  TSV_HEADER  JSON  NULL
```

`JSON` emits one JSON object per result row; `NULL` discards result rows. `AUTO`
uses `COLUMNS` when set and chooses vertical output when the aligned table does
not fit. The formats cover common presentation and machine-output workflows;
they are not a claim of full byte-for-byte Trino formatter parity.

## Embedded local mode

From the repository root:

```bash
cd engine
cargo run -p kaveon-cli -- --data-dir /path/to/parquet
```

On Windows, use a Windows path:

```powershell
cd engine
cargo run -p kaveon-cli -- --data-dir D:\data\warehouse
```

The positional form is equivalent:

```bash
cargo run -p kaveon-cli -- /path/to/parquet
```

For a reusable binary, run `cargo build -p kaveon-cli --release`. The executable
is written to `engine/target/release/kaveon` (`kaveon.exe` on Windows).

Use `kaveon --help` for startup options and `kaveon --version` for the installed
version.

## Quick mode: discover one directory

`--data-dir` (or `-d`) scans the named directory for immediate, lowercase
`*.parquet` files and immediate Delta table directories. Each readable Parquet
file becomes one table in the `default` schema; the filename without `.parquet`
is the table name. A child directory containing `_delta_log` becomes a Delta
table named after that directory. Discovery is not recursive beyond inspecting
those immediate Delta table directories.

For example, `/data/sales.parquet` is queried as `sales`:

```sql
SELECT region, SUM(revenue)
FROM sales
GROUP BY region;
```

SQL statements may span multiple lines and execute when the CLI receives a
trailing semicolon.

## Catalog files

For multiple logical catalogs, use the default Trino-like layout:

```text
~/.kaveon/
├── config.toml
└── catalogs/
    ├── sales.toml
    └── operations.toml
```

The catalog name comes from the catalog filename. This `config.toml` selects the
defaults used for unqualified table names:

```toml
default_catalog = "sales"
default_schema = "default"
```

Configure a local catalog in `~/.kaveon/catalogs/sales.toml`:

```toml
type = "local"
base_path = "/data/sales"
```

On Windows, for example:

```toml
type = "local"
base_path = "D:\data\sales"
```

When a local catalog has no explicit `[[table]]` entries, the CLI discovers the
immediate `*.parquet` files in `base_path` and registers them in the `default`
schema.

To choose table names, schemas, or file locations explicitly, add entries such as:

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
name = "customers"
schema = "commerce"
location = "customers.parquet"
access = "shortcut"
format = "parquet"
```

Each explicit `location` is resolved relative to `base_path`. For a Parquet
table, `location` names one file. For a Delta table, it names the table directory:

```toml
[[table]]
name = "order_events"
schema = "commerce"
location = "order_events"
access = "shortcut"
format = "delta"
```

Local Delta reads replay the JSON commits in `_delta_log` to determine the active
Parquet files and then read all active files. The reader requires a complete JSON
commit history beginning at version 0. Checkpoint replay is not implemented, so
a checkpoint-only or otherwise incomplete JSON history is rejected instead of
returning a partial snapshot.

Although catalog types and configuration values exist for cloud storage and
Iceberg, ADLS Gen2, S3, and Iceberg reads are not executable today.

Start with the default configuration:

```bash
kaveon
```

Or select another main configuration path:

```bash
kaveon --config /path/to/config.toml
```

When a `catalogs` directory exists next to the selected configuration file, the
CLI loads its `.toml` and `.properties` files as separate catalogs. Otherwise,
the CLI can load the older single-file `[[catalog]]` layout shown in
`engine/kaveon.example.toml`.

## Local catalog commands

In local mode, these commands end with a semicolon:

| Command | Purpose |
|---|---|
| `SHOW CATALOGS;` | List registered catalogs |
| `SHOW SCHEMAS;` | List schemas in the default catalog |
| `SHOW SCHEMAS FROM sales;` | List schemas in a named catalog (`IN` is also accepted) |
| `SHOW TABLES;` | List tables in the default catalog and schema |
| `SHOW TABLES FROM sales.commerce;` | List tables in a catalog and schema (`IN` is also accepted) |
| `DESCRIBE sales.commerce.orders;` | Show column names, Arrow types, and nullability |
| `DESC sales.commerce.orders;` | Short form of `DESCRIBE` |

Table references may be unqualified (`orders`), schema-qualified
(`commerce.orders`), or fully qualified (`sales.commerce.orders`). Defaults from
`config.toml` supply omitted catalog and schema names.

The CLI also provides dot commands, which execute immediately without a
semicolon:

| Command | Purpose |
|---|---|
| `.catalogs` | List catalogs and mark the default |
| `.schemas [catalog]` | List schemas in the default or named catalog |
| `.tables [schema]` | List tables in a schema of the current catalog |
| `.describe <table>` or `.desc <table>` | Show table metadata and columns |
| `.use <catalog.schema>` | Switch the default catalog and schema |
| `.help` or `.h` | Show CLI help |
| `.quit`, `.exit`, or `.q` | Exit |

`USE catalog.schema;` and `.use catalog.schema` validate the target and switch
defaults without unloading registered catalogs. `USE catalog;` preserves the
current default schema and validates that it exists in the selected catalog.

## Verify a local catalog

After starting the shell, use a short discovery and query sequence:

```sql
SHOW CATALOGS;
SHOW SCHEMAS FROM sales;
SHOW TABLES FROM sales.commerce;
DESCRIBE sales.commerce.orders;

SELECT COUNT(*)
FROM sales.commerce.orders;

SELECT customer_id, SUM(total_amount) AS revenue
FROM sales.commerce.orders
GROUP BY customer_id
LIMIT 10;
```

The CLI prints query results, elapsed time, and any parsing, planning, or execution
error in the terminal.

On a remote coordinator with durable product-catalog storage enabled, an Engine
administrator can refresh snapshot-bound optimizer statistics:

```sql
ANALYZE OpenSource.ai_benchmarks.leaderboard;
```

The command returns the qualified table and exact metadata row count. Kaveon
records the resolved catalog identity and the Delta version, Iceberg snapshot,
or Parquet object identity in the same conditional ADLS catalog publication.
The planner ignores the result whenever either identity no longer matches.
`ANALYZE` is unavailable in embedded `--local` mode and to reader or analyst
roles.

## Current boundaries

- `--local` embeds and executes the Engine in its own process. It does not submit
  statements to `kaveon-server`, so those local queries do not appear in Engine
  query history. Remote history is process-local and bounded; it is not a
  durable audit log.
- Queries submitted through the Engine HTTP API appear immediately as running
  records. Completed records retain measured lifecycle timings, the logical
  plan, and Parquet/Delta scan statistics. Physical operator and distributed
  stage/task metrics are explicitly unavailable until executor instrumentation
  is wired.
- The Engine currently executes local Parquet and local Delta Lake reads. Iceberg,
  ADLS Gen2, and S3 are not executable through the CLI.
- A plain Parquet table is one configured file. Reading multiple Parquet files as
  one table is supported through a local Delta table whose active files are
  recorded in a complete JSON transaction history.
- SQL support is intentionally narrower than Trino. See
  [Engine SQL compatibility](../reference/engine-sql-compatibility.md) before
  relying on joins, sorting, DDL, or DML.
- The client does not provide query-history editing. `SHOW TABLES ... LIKE` is
  supported for remote catalog metadata. It does not claim full Trino command,
  session-property, wire-protocol, or SQL parity.
