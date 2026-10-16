# Track 2 AKS: current-image `FETCH … PERCENT` wire qualification

## Result

The immutable Engine image `sha256:60de4cf526c2340959fcd0ce264c9c4cca1680ced8d462617c4287d93e01f8c9` passed the complete twelve-shape Trino wire corpus twice. The new `FETCH FIRST 50 PERCENT ROWS ONLY` case returned 12 rows on both runs, finished successfully, and matched exactly. The existing eleven shapes also remained exact and successful.

The run used the AKS coordinator with three catalog-compatible workers and authenticated TLS. No credentials are retained in the raw report.

The corpus covers non-recursive CTEs, aggregate `FILTER`, `GROUPING SETS`, window frames, `FETCH … WITH TIES`, literal `FETCH … PERCENT`, scalar subqueries, array/map/JSON expressions, `UNNEST … WITH ORDINALITY`, constant `CROSS JOIN LATERAL UNNEST`, `EXISTS`, and `UNION ALL`.

The raw report is retained in
`track2-aks-wire-corpus-fetch-percent-current-2026-10-16.json`.

## Boundary

This qualifies the declared wire shapes on the three-worker AKS topology. Correlated/lateral table functions, recursive CTEs, broader correlated `Apply` forms, complete JSON coercion/path semantics, general DML, and broader external-client differential coverage remain open in the parity tracker.