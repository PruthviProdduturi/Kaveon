# Query estimate-vs-actual diagnostic — 2026-10-29

Status: **qualified for the bounded query-level contract**.

`GET /v1/query/{query_id}/estimate-diagnostics` compares the source-row
estimates attached to optimized `Scan` nodes with the rows selected by the
finished query's scan telemetry. It reports the estimate sources, ratio, and
one conservative classification: `within_2x`, `underestimated`, or
`overestimated`.

Queries without both plan estimates and completed scan telemetry return
`available: false`; this includes cache/context answers and plan-only requests.

## Evidence

- Unit coverage: `estimate_diagnostic_collects_versioned_scan_rows`.
- Existing plan-estimate coverage verifies the versioned source metadata used
  by the diagnostic.
- Workspace server compilation and strict Clippy pass.

## Boundary

This is query-level evidence aggregated across scans. Per-operator actual
cardinality, selectivity attribution, and release-threshold automation remain
open and are not implied by this endpoint.
