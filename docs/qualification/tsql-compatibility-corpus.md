# Local Fabric/T-SQL Compatibility Corpus

Status: active qualification corpus for Track A  
Environment: local Docker coordinator plus two workers  
Source of truth: the SQL planner, executor, and distributed query result

The latest local runtime report is
[`tsql-compatibility-local-2026-10-28.json`](tsql-compatibility-local-2026-10-28.json).
It runs the live coordinator and both workers twice per case and records client
and server latency without storing full result sets.

This corpus separates planner lowering checks from live queries executed against
Kaveon. A feature is promoted to the compatibility matrix only after its local
planner/executor tests pass and, where a runtime case is listed below, its local
and distributed results agree.

## Verified now

| ID | Query shape | Expected result or behavior | Local | Distributed |
|---|---|---|---:|---:|
| TSQL-001 | `SELECT TOP 2 value FROM sample` | bounded `Limit` plan | plan test | plan test |
| TSQL-002 | `SELECT TOP (2) value FROM sample` | bounded `Limit` plan | plan test | plan test |
| TSQL-003 | `SELECT ISNULL(value, 0) FROM sample` | lowered to vectorized `COALESCE` | plan test | plan test |
| TSQL-004 | `SELECT DATEADD(day, 1, DATE '2024-01-31')` | epoch-day `19754` | pass | pass |
| TSQL-005 | `SELECT DATEADD(month, 1, DATE '2024-01-31')` | month-end clamp to `2024-02-29` | pass | pass |
| TSQL-006 | `SELECT DATEPART(year, DATE '2024-01-31')` | `2024` | pass | pass |
| TSQL-007 | `SELECT DATEDIFF(day, DATE '2024-01-31', DATE '2024-02-02')` | `2` | pass | pass |
| TSQL-008 | `SELECT CONVERT(INT, '42')` | `42` | pass | pass |
| TSQL-009 | `SELECT TRY_CONVERT(INT, 'bad')` | `NULL`, no query failure | pass | pass |
| TSQL-010 | `SELECT DATEPART(year, DATE '2024-01-01'), ROW_NUMBER() OVER (ORDER BY trips DESC) FROM OpenSource.public.nyc_taxi_borough` | correct date part and row numbers | pass | pass |
| TSQL-011 | Date functions in a numeric join, `WHERE`, `GROUP BY`, `HAVING`, and `ORDER BY` over `OpenSource.climate_energy.energy_annual` | correct grouped rows and date-part value | pass | pass |
| TSQL-012 | `SELECT TOP (2) [year] FROM OpenSource.climate_energy.energy_annual ORDER BY [year]` | bounded result with bracketed identifier | pass | pass |
| TSQL-013 | `DATEADD` and `DATEDIFF` over timestamp literals crossing midnight | `2024-02-01T00:00:00`, `2` hours | pass | pass |
| TSQL-014 | `YEAR`, `MONTH`, and `DAY` over a leap-day literal | `2024`, `2`, `29` | pass | pass |
| TSQL-015 | `GETDATE()` and `GETUTCDATE()` | one current UTC timestamp row | pass | pass |
| TSQL-016 | `QUALIFY` over `ROW_NUMBER()` on the local borough table | two qualified rows | pass | pass |
| TSQL-017 | `DATEADD` year/quarter/week/minute/second across leap-day and midnight boundaries | exact calendar and timestamp results | pass | pass |
| TSQL-018 | `CONVERT` to `BIGINT`, `FLOAT`, and `VARCHAR`; invalid `TRY_CONVERT(DATE, …)` | exact values plus `NULL` for invalid input | pass | pass |
| TSQL-019 | Nullable `DATEADD` delta and nested `CONVERT(INT, CAST(NULL AS VARCHAR))` | NULL propagation without worker failure | pass | pass |
| TSQL-020 | Uncorrelated `CROSS APPLY` over a scalar derived relation | distributed cross-join semantics | pass | pass |
| TSQL-021 | Uncorrelated `CROSS APPLY` over a two-row `VALUES` relation | distributed cross product and alias binding | pass | pass |
| TSQL-022 | DATEADD plus exact count/sum over the 10.12M-row dashboard fact table | distributed aggregate with temporal projection | pass | pass |
| TSQL-023 | DATEPART plus grouped SUM and ORDER BY over the 10.12M-row dashboard fact table | distributed grouped aggregate and deterministic top-three ordering | pass | pass |
| TSQL-024 | Negative DATEADD day/month offsets across leap-day and month-end boundaries | exact calendar arithmetic and month-end clamp | pass | pass |
| TSQL-025 | CONVERT and TRY_CONVERT of NULL typed inputs | NULL result propagation without execution failure | pass | pass |
| TSQL-026 | DATEPART quarter plus month/year DATEDIFF across leap-year boundaries | exact calendar-unit semantics | pass | pass |
| TSQL-027 | NULL DATEADD source plus invalid TRY_CONVERT(DATETIME2, …) | NULL propagation across temporal kernels | pass | pass |
| TSQL-028 | DATEADD with a BIGINT delta | integer-width normalization in the temporal kernel | pass | pass |
| TSQL-029 | Literal U.S. date through `CONVERT(DATE, …, 101)` | style-aware Date32 normalization | pass | pass |
| TSQL-030 | Two-digit literal U.S. date through `CONVERT(DATE, …, 0)` | numeric style-0 normalization | pass | pass |
| TSQL-031 | Textual month literal through `CONVERT(DATE, …, 0)` | style-0 month-name normalization | pass | pass |
| TSQL-032 | Invalid textual style-0 date through `TRY_CONVERT` | NULL-on-error conversion semantics | pass | pass |
| TSQL-033 | Valid textual `DATETIME2` conversion | timestamp parsing and client serialization | pass | pass |
| TSQL-034 | Grouped platform SUM/COUNT over the 10.12M-row dashboard fact table | distributed grouped aggregate and deterministic ordering | pass | pass |
| TSQL-035 | Composite region/platform grouped SUM over the 10.12M-row dashboard fact table | distributed two-key hash grouping and deterministic top-five ordering | pass | pass |
| TSQL-036 | Grouped platform SUM with a HAVING threshold over the 10.12M-row dashboard fact table | distributed post-aggregate filtering | pass | pass |
| TSQL-037 | Exact distinct user and region counts over the 10.12M-row dashboard fact table | distributed distinct aggregation | pass | pass |
| TSQL-038 | Per-region exact distinct user counts over the 10.12M-row dashboard fact table | grouped distinct aggregation and deterministic ordering | pass | pass |
| TSQL-039 | Per-region exact distinct counts with a HAVING threshold over the 10.12M-row dashboard fact table | distributed distinct post-aggregate filtering | pass | pass |

## Explicitly rejected

| ID | Query shape | Required behavior |
|---|---|---|
| TSQL-R01 | `TOP PERCENT` | actionable unsupported error |
| TSQL-R02 | `TOP WITH TIES` | use `FETCH WITH TIES` instead |
| TSQL-R03 | `ISNULL(a, b, c)` | strict two-argument error |
| TSQL-R04 | dynamic `DATEADD(datepart_column, ...)` | datepart must be a literal token |
| TSQL-R05 | unsupported datepart such as `millisecond` | actionable unsupported-unit error |
| TSQL-R06 | unsupported `CONVERT` style or character encoding | actionable style/encoding error |

## Next corpus additions

### Date and conversion cases

- Negative offsets and additional leap-year boundaries beyond TSQL-017
- `DATEDIFF` for month, quarter, year, hour, minute, and second
- `DATEPART` for year, quarter, month, week, day, weekday, and day-of-year
- `CONVERT` styles and `DATETIME2` precision beyond TSQL-018
- Additional invalid numeric and temporal `TRY_CONVERT` cases
- Null input/result propagation for nested expressions beyond TSQL-019

### Relational placement cases

- Date functions in projection
- Date functions in `WHERE`
- Date functions in `GROUP BY`
- Date functions in `HAVING`
- Date functions in `ORDER BY`
- Date functions inside window expressions
- Date functions in join predicates

### Future compatibility cases

- Correlated `CROSS APPLY` (lateral binding is not yet implemented).
- `OUTER APPLY` (null-extension semantics are not yet implemented).
- `PIVOT`
- `UNPIVOT`
- T-SQL alias and identifier edge cases

These remain unpromoted until planner and executor tests exist. `MERGE`,
procedural batches, stored procedures, cursors, triggers, and broad T-SQL DDL
remain outside this corpus and are explicitly unsupported.

### Promotion gates for the next tier

- `PIVOT`: static value lists must lower to conditional aggregates while
  preserving non-pivot grouping columns, aggregate aliases, NULL/default-on-
  null behavior, and distributed merge equivalence. Dynamic `ANY` and
  subquery value sources stay rejected until their output schema is bounded.
- `UNPIVOT`: static column lists must lower to typed `UNION ALL` branches with
  stable name/value columns, NULL filtering matching the dialect contract, and
  distributed row-count equivalence.
- Correlated `CROSS APPLY`: lateral outer-column binding must be explicit and
  tested for per-row evaluation; uncorrelated APPLY is already promoted by
  TSQL-020 and TSQL-021.
- `OUTER APPLY`: null-extension for empty right-side results must be tested
  separately from ordinary LEFT JOIN behavior.

## Commands

Planner and executor tests:

```powershell
cd engine
cargo test -p kaveon-sql --all-targets
cargo test -p kaveon-exec --lib date_add
```

Local runtime smoke:

```powershell
docker compose up -d engine-coordinator engine-worker-1 engine-worker-2
```

Submit the queries through `POST http://127.0.0.1:8081/v1/statement` with
`catalog=OpenSource`, the appropriate schema, and `result_delivery=inline`.

## Qualification rule

Do not mark a row as verified from a planner debug string alone. The query must
finish through the live coordinator and workers, return the expected values,
and preserve the same semantics in local and distributed execution.
