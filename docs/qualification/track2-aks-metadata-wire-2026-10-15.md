# Track 2: metadata protocol wire qualification on AKS

**Date:** 2026-10-15  
**Environment:** AKS `kaveon-aks`, namespace `kaveon`  
**Engine image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`

The release image was exercised through `/v1/trino/statement` with Trino
session headers and an authenticated operator principal. Each request returned
HTTP 200 and the response pages were drained to completion.

| Statement | Pages | Rows | Result |
|---|---:|---:|---|
| `SHOW SESSION` | 1 | 5 | Exact supported settings returned |
| `SHOW SESSION LIKE 'result%'` | 1 | 1 | Pattern filtering returned `result_cache` |
| `SHOW FUNCTIONS LIKE 'array%'` | 1 | 5 | Function metadata returned |
| `SHOW CATALOGS` | 1 | 13 | Catalog visibility returned |
| `EXPLAIN SELECT year FROM climate_energy.energy_annual ORDER BY year LIMIT 2` | 1 | 3 | Plan rows returned |

The query-result rows and all `nextUri` pages were read successfully; no
protocol error or worker failure occurred. The `SHOW SESSION` check is
significant because the native JSON statement route intentionally accepts only
query statements, while Trino clients use the dedicated wire adapter for this
control statement.

This qualifies the bounded metadata wire surface. Full external-client
compatibility and the broader SQL/complex-type matrix remain open in the parity
tracker.
