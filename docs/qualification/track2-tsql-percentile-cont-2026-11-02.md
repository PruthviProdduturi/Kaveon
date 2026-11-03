# Track 2 qualification — bounded ordered percentiles

## Scope

Kaveon now accepts these bounded SQL Server/Fabric analytic forms:

```sql
PERCENTILE_CONT(0.5)
  WITHIN GROUP (ORDER BY value)
  OVER (PARTITION BY region)
```

```sql
PERCENTILE_DISC(0.5)
  WITHIN GROUP (ORDER BY value)
  OVER (PARTITION BY region)
```

The fraction must be a finite numeric literal in `[0, 1]`. Exactly one numeric
`WITHIN GROUP` key is required; the window `ORDER BY` clause cannot be supplied
in addition to it. NULL values are ignored and an all-NULL partition returns
NULL. `PERCENTILE_CONT` uses continuous linear interpolation and a `DOUBLE`
output; `PERCENTILE_DISC` returns the selected non-NULL value in its input
type.

## Exact qualification

`distributed_tsql_percentile_cont_matches_local` and
`distributed_tsql_percentile_disc_matches_local` compare the local planner
with the three-worker fragment path over partitioned `actions` values. The
complete partition result, including every repeated median value, matched
exactly and the query memory pool returned to zero after both executions.

Planner tests also reject dynamic fractions, multiple order keys, missing
`OVER`, and conflicting window ordering. This prevents the bounded contract
from silently widening into unsupported ordered-set semantics.

## Boundary

This does not claim dynamic fractions, multiple ordering keys, explicit NULLS
placement, or external-client/cloud qualification.
