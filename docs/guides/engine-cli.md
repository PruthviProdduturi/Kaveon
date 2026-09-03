# Kaveon Engine CLI

Status: **Alpha**. The Kaveon CLI is an interactive SQL shell for the standalone
Rust Engine. Its catalog navigation follows familiar Trino commands, while query
execution currently reads local Parquet files and local Delta Lake tables.

## Build and start the CLI

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

## Trino-familiar commands

These commands end with a semicolon:

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

## Verify a catalog

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

## Current boundaries

- The CLI embeds and executes the Engine in its own process. It does not submit
  statements to `kaveon-server`, so CLI queries do not appear in the Engine web
  UI query history.
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
