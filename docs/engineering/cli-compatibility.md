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
| `USE [catalog.]schema` | Validates context before switching; selected schema appears in prompt |
| `.catalogs`, `.schemas`, `.tables`, `.use` | Existing shortcuts retained |
| `help`, `exit`, `quit`, `clear` | Shell commands, optional semicolon; clear applies to an interactive terminal |
| Metadata and query tables | Aligned text/numeric output and row count |
| Query summary | Actual query ID, state, elapsed time and returned rows |
| `--execute`, JSON, CSV and TSV | Supported; metadata respects output format |
| External pager | No dependency on `less`; output is printed directly |
| Up/down history, persistent history, completion and editing modes | Pending |
| Multiple statements in a batch/file | Pending |
| Trino progress, split/node statistics, advanced output formats and spooling | Pending; no invented statistics |
| Full Trino session properties, authentication mechanisms and SQL commands | Not claimed; requires separate Engine and client work |

`SHOW` and `USE` are interpreted by the remote CLI. The current Engine SQL HTTP
endpoint still has its own supported SQL surface. Unsupported metadata clauses
must report errors instead of being silently ignored. Failed context changes
retain the previous catalog/schema, and interactive command errors keep the
session open.

The Engine UI labels queries using their reported client metadata (for example
`kaveon-cli` becomes **Kaveon CLI**). Its **User** field comes from authenticated
server identity, with an immutable principal retained separately for ownership
and audit. A submitted CLI username does not grant access or replace that identity.
