# SQL Server `COALESCE` qualification — 2026-11-03

## Boundary

Kaveon qualifies the bounded SQL Server-compatible `COALESCE` read form for
row-local arguments. It preserves left-to-right NULL fallback and uses the
existing typed expression coercion path, including a typed NULL followed by a
numeric column and fallback literal. This is a read-compatibility claim; it
does not claim SQL Server's full type-precedence matrix for every user type.

## Evidence

The differential case `tsql_coalesce` runs:

```sql
SELECT user_id,
       COALESCE(country, 'unknown'),
       COALESCE(CAST(NULL AS VARCHAR), country),
       COALESCE(CAST(NULL AS BIGINT), actions, 0)
FROM {T}
ORDER BY user_id, actions, country
LIMIT 20
```

It compares exact results across dictionary, plain, fragmented,
partitioned, clustered, and three-worker execution paths. The focused run
passed on Engine commit `49b12eb3`.

The case is recorded as corpus entry `coalesce` in
`docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.

## Non-claims

Dynamic type-dependent expressions, user-defined types, and arbitrary
cross-source coercion remain outside this bounded qualification.
