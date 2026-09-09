# CLI compatibility checkpoint

This compares Kaveon's remote CLI with the everyday workflows documented in the
[Trino CLI guide](https://trino.io/docs/current/client/cli.html). It is not a claim
of complete CLI, wire-protocol or SQL-engine parity.

| Workflow | Kaveon status |
|---|---|
| Connect to HTTPS with Azure login | Supported; Engine-scoped token acquired automatically |
| `SHOW CATALOGS` | CLI metadata command over authenticated catalog APIs |
| `SHOW SCHEMAS IN/FROM catalog` | CLI metadata command |
| `SHOW TABLES IN/FROM [catalog.]schema` | CLI metadata command |
| `SHOW ... LIKE 'pattern'` | Client-side `%`/`_` filtering over authenticated metadata names |
| `DESCRIBE` / `SHOW COLUMNS FROM` | Catalog-definition metadata: column name, type, and nullability |
| `USE [catalog.]schema` | Validates context before switching; selected schema appears in prompt |
| `.catalogs`, `.schemas`, `.tables`, `.use` | Existing shortcuts retained |
| `help`, `exit`, `quit`, `clear` | Shell commands, optional semicolon; clear applies to an interactive terminal |
| Metadata and query tables | Aligned text/numeric output and row count |
| Query summary | Query ID/state, elapsed time, distinct recorded execution nodes, task completion, returned rows/JSON result bytes and result rates |
| `--execute` and `--file` | Supported; a UTF-8 script runs statements in order; `--ignore-errors` continues after failures with a nonzero exit |
| Persistent history, completion and editing modes | Supported in the interactive terminal; `--history-file`, `--no-history`, and `EMACS`/`VI` editing are available |
| Connection invocation and defaults | Positional coordinator URL with optional `/catalog/schema`, `--option=value`, duration request timeout, and an allowlisted `KAVEON_CONFIG`/`~/.kaveon_config` defaults file |
| Explicit bearer token | `--access-token`; `KAVEON_ACCESS_TOKEN` is preferable for unattended use |
| Output formats | Legacy lowercase `table`, `csv`, `tsv`, `json`, plus uppercase `ALIGNED`, `VERTICAL`, `AUTO`, `MARKDOWN`, CSV/TSV variants, JSON Lines, and NULL |
| External pager | Optional `--pager`; an empty value disables pagination |
| Scan-reader counters | Supported for complete Parquet worker coverage; reader-output rows, selected row-group rows, and selected compressed bytes are measured rather than inferred from results |
| Live progress, full Trino-style split statistics, and spooling | Pending; recorded Kaveon stage tasks are labeled tasks |
| Full Trino session properties, authentication mechanisms and SQL commands | Not claimed; requires separate Engine and client work |

`SHOW` and `USE` are interpreted by the remote CLI. The current Engine SQL HTTP
endpoint still has its own supported SQL surface. Unsupported metadata clauses
must report errors instead of being silently ignored. Failed context changes
retain the previous catalog/schema, and interactive command errors keep the
session open.

Human-readable output ends with a blank line before the next prompt. Lowercase
machine formats retain their existing serialization. Uppercase `JSON` emits one
JSON object per row and `NULL` emits no result rows. `AUTO` uses `COLUMNS` when
available and otherwise assumes 120 columns before choosing aligned or vertical
output. These names support common Trino CLI workflows but do not claim exact
Trino formatter byte compatibility. The table-mode footer reads query
telemetry after execution; a missing/expired telemetry record does not turn a
successful query into an error. Nodes are distinct recorded task nodes, not all
available cluster nodes. Result bytes describe compact JSON serialization of the
returned data, not storage bytes scanned. Result rates use total query elapsed
time. Complete Parquet worker coverage reports measured reader-output rows,
selected row-group rows, and selected compressed bytes. Delta/Iceberg or
mixed-version coverage is marked unavailable rather than shown as zero.
Metadata-only CLI commands do
not fabricate Engine query IDs or execution statistics.

The Engine UI labels queries using their reported client metadata (for example
`kaveon-cli` becomes **Kaveon CLI**). Its **User** field comes from authenticated
server identity, with an immutable principal retained separately for ownership
and audit. A submitted CLI username does not grant access or replace that identity.
