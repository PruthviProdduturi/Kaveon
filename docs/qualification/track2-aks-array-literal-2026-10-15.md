# Track 2: distributed array-literal regression on AKS

**Date:** 2026-10-15  
**Environment:** AKS `kaveon-aks`, namespace `kaveon`  
**Engine image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`

The reported failure shape was a projection containing an array expression;
the old deployment returned `unknown scalar function: ARRAY` from workers. The
current immutable image was exercised through the authenticated Trino wire
endpoint with:

```sql
SELECT DISTINCT year
FROM climate_energy.energy_annual
ORDER BY year
```

The statement completed with HTTP 200 and `FINISHED` state in 1,598 ms. It
returned the exact six rows `2020, 2021, 2022, 2023, 2024, 2025`, with no worker
error. The query crossed the distributed execution path; it was not a local
unit-test-only result.

Focused implementation evidence also passes: 65 array-focused `kaveon-exec`
tests and 13 array-focused `kaveon-sql` tests completed with zero failures.

This closes the reported deployment regression for this query shape. It does
not claim full Trino complex-type parity; broader array, map, row, JSON, and
lateral/table-function qualification remains tracked in the parity matrix.
