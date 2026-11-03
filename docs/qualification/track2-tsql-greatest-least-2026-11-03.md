# T-SQL `GREATEST`/`LEAST` numeric qualification — 2026-11-03

This record qualifies the bounded SQL Server/Fabric `GREATEST` and `LEAST`
read spellings for non-null numeric expressions. The Engine preserves the
common numeric type selected by its normal coercion path and evaluates both
functions in local and three-worker execution.

## Scope

Qualified statements:

```sql
SELECT GREATEST(actions, sessions, duration_sec)
FROM events;

SELECT LEAST(actions, sessions, errors)
FROM events;
```

The qualification fixture uses non-null `BIGINT` columns. NULL-bearing
arguments remain outside this SQL Server/Fabric compatibility claim because
SQL Server and Trino differ in their NULL handling for these functions; the
Engine's documented common-function contract remains explicit rather than
silently claiming both dialects.

## Evidence

- `engine/crates/server/src/differential_tests.rs`:
  `distributed_tsql_greatest_and_least_match_local_numeric_semantics`
- exact local-versus-three-worker result comparison over the deterministic
  events fixture;
- query memory pool returned to zero after both paths;
- corpus entries `greatest_numeric` and `least_numeric` in
  `docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.

This is Engine execution evidence for a bounded read surface. It is not a
claim of general SQL Server/Fabric function parity, NULL-semantic parity, or
external-client qualification.
