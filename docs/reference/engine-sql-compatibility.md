# Engine SQL Compatibility

Status: **Alpha**. This page describes the standalone Rust Engine, not SQL sent by
Studio to registered databases. Registered-source SQL compatibility is determined
by the selected database and Kaveon's API guardrails.

## Current Engine support

| SQL feature | State | Notes |
|---|---|---|
| `SELECT` | Alpha | Local Parquet and local Delta tables resolved through the Engine catalog |
| Column projection and aliases | Alpha | Projection is strict; unknown or duplicate requested columns fail |
| `WHERE` comparisons and boolean expressions | Alpha | Row-level filter operator is implemented |
| `GROUP BY` | Alpha | In-memory blocking hash aggregation |
| `SUM`, `COUNT`, `AVG`, `MIN`, `MAX` | Alpha | No spill or distributed partial aggregation |
| `LIMIT` | Alpha | Physical limit operator |
| Qualified table names | Alpha | `catalog.schema.table` resolution is supported by the catalog |
| `ORDER BY` | Parsed only | Physical planners currently pass through the `Sort` node; results are not sorted |

## Not currently executable

- Joins, subqueries, window functions, `HAVING`, set operations, DDL, and DML.
- Physical Sort or TopN execution.
- Filter predicate pushdown from the logical planner into Parquet row-group pruning.
- Delta checkpoint replay, Iceberg, ADLS Gen2, or S3 reads. Local Delta tables
  are supported only when their complete JSON commit history is available from
  version 0.
- Cross-node fragments, shuffle, retry, spill, admission control, or cancellation.

Unsupported syntax should be treated as unsupported even if the upstream SQL
parser accepts it. The executable contract is the intersection of parsing,
logical planning, and physical operator construction.

## Current execution semantics

`POST /v1/statement` executes synchronously and materializes the complete result in
memory before returning JSON. Query IDs identify retained completed/failed records;
`DELETE /v1/query/{id}` deletes a record and does not cancel running work.

See [Architecture](../../ARCHITECTURE.md), [HTTP API](api.md), and
[Operations and troubleshooting](../operations-troubleshooting.md).
