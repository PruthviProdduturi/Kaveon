# SQL Server `NULLIF` qualification — 2026-11-03

## Boundary

Kaveon qualifies the row-local SQL Server `NULLIF(value, comparison)` read
form. Equal non-NULL values produce typed NULL; unequal values preserve the
first argument, and NULL inputs retain the existing three-valued,
null-preserving expression semantics. This is a bounded scalar-read claim,
not a claim about arbitrary subquery or volatile-expression evaluation order.

## Evidence

The differential case `tsql_nullif` runs:

```sql
SELECT user_id, NULLIF(actions, 0)
FROM {T}
ORDER BY user_id, actions
LIMIT 20
```

The case compares exact local and three-worker results across dictionary,
plain, fragmented, partitioned, and clustered Parquet layouts. The focused
case is part of the full server release suite, which passed 697/697 tests.

The case is recorded as corpus entry `nullif` in
`docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.

## Non-claims

Subquery-valued arguments, volatile expressions, and cross-source coercion
remain outside this bounded qualification.
