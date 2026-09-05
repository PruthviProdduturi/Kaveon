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
| `GROUP BY` | Alpha | Local and distributed partial/final hash aggregation; aggregate spill absent |
| `SUM`, `COUNT`, `AVG`, `MIN`, `MAX`, `COUNT(DISTINCT ...)` | Alpha | Weighted AVG and exact DISTINCT states are mergeable across workers |
| `LIMIT` | Alpha | Physical limit operator |
| Qualified table names | Alpha | `catalog.schema.table` resolution is supported by the catalog |
| `ORDER BY`, `NULLS FIRST/LAST`, TopN | Alpha | Local/distributed execution; Sort/TopN support bounded external merge runs |
| Equi joins | Alpha | INNER/LEFT/RIGHT/FULL locally; distributed hash repartition path implemented |
| Cross joins | Alpha | Local Cartesian semantics and distributed broadcast-build path |
| Arithmetic expressions | Alpha | Compatible numeric coercion for supported primitive numeric types |
| `HAVING` | Alpha | Post-aggregation filter |
| Window functions | Alpha | `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, aggregates with `OVER (PARTITION BY … ORDER BY …)` |
| Set operations | Alpha | `UNION ALL`, `INTERSECT`, `EXCEPT` |
| Date/time functions | Alpha | `EXTRACT`, `DATE_TRUNC`, `DATE_PART`, `TO_CHAR`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP` |
| Conditional and comparison | Alpha | `CASE`, `COALESCE`, `BETWEEN`, `IN`, `LIKE`, `ILIKE`, `CAST` |
| `SUM(DISTINCT)`, `AVG(DISTINCT)` | Alpha | Exact mergeable distinct state |

## Not currently executable

- Scalar and correlated subqueries.
- Non-equality join conditions and broader join expressions.
- DDL and DML. The Engine reads; it does not create or mutate tables.
- Delta checkpoint replay, Iceberg, ADLS Gen2, or S3 reads. Local Delta tables
  are supported only when their complete JSON commit history is available from
  version 0.
- Admission control, resource groups, aggregate/join spill, cost-based optimization, dynamic filtering, and exchange streaming flow control.

Unsupported syntax should be treated as unsupported even if the upstream SQL
parser accepts it. The executable contract is the intersection of parsing,
logical planning, and physical operator construction.

## Current execution semantics

`POST /v1/statement` returns synchronously and materializes the root result before JSON serialization. Internally, eligible plans run as dependency-gated stages with versioned worker fragments and Arrow IPC exchanges. Query IDs identify retained records; cancellation propagates to active worker tasks, but history remains process-local.

See [Architecture](../../ARCHITECTURE.md), [HTTP API](api.md), and
[Operations and troubleshooting](../operations-troubleshooting.md).
