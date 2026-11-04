# T-SQL `STRING_SPLIT` — 2026-10-29

## Scope

Kaveon accepts the bounded SQL Server table-valued shape:

```sql
STRING_SPLIT(string_expression, separator_expression [, enable_ordinal])
```

It lowers to the existing typed `SPLIT` expression and `UNNEST` operator.
The optional `enable_ordinal` argument must be the literal `0` or `1`; `1`
adds a one-based `ordinal` column. The relation alias may rename `value` and,
when enabled, `ordinal`. Correlated inputs require `CROSS JOIN LATERAL` and
use the existing lateral execution path. Arbitrary table-valued functions,
dynamic ordinal modes, and broader SQL Server collation behavior remain out of
scope.

## Evidence

- Planner coverage was included in the versioned 100-case historical T-SQL
  corpus revision; the current authoritative corpus contains 187 cases.
- `OUTER APPLY STRING_SPLIT(...)` is qualified as a bounded left-lateral
  lowering alongside `CROSS APPLY` and explicit `CROSS JOIN LATERAL`.
- Focused three-worker differential:
  `differential_tests::distributed_tsql_string_split_matches_local`.
- The full Parquet-layout and execution-path differential sweep passes
  value-only, ordinal, and row-local lateral forms with deterministic ordering.

Scalar row-local string and separator expressions are supported when the
function is invoked through `CROSS JOIN LATERAL` or the bounded SQL Server
`CROSS APPLY STRING_SPLIT(...)` spelling; the ordinal mode remains a literal
`0` or `1`. NULL propagation and delimiter behavior are inherited from the qualified
vectorized `SPLIT` evaluator; the row-producing boundary adds only the
SQL Server-compatible relation and ordinal contract described above.
