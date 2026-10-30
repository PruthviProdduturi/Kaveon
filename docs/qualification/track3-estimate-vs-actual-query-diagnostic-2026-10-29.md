# Query estimate-vs-actual diagnostic — 2026-10-29

Status: **qualified for the bounded query-level and ordinal scan-attribution contract**.

`GET /v1/query/{query_id}/estimate-diagnostics` compares the source-row
estimates attached to optimized `Scan` nodes with the rows selected by the
finished query's scan telemetry. It reports the estimate sources, ratio, and
one conservative classification: `within_2x`, `underestimated`, or
`overestimated`.

Queries without both plan estimates and completed scan telemetry return
`available: false`; this includes cache/context answers and plan-only requests.
When the optimized plan and completed scan telemetry expose the same scan
cardinality, the response also includes `per_scan` entries with stable plan
node IDs, estimated rows, selected rows, estimate source, ratio, and the same
conservative classification. If the scan counts cannot be matched exactly,
`per_scan` is omitted rather than guessing attribution.

## Evidence

- Unit coverage: `estimate_diagnostic_collects_versioned_scan_rows` and
  `estimate_diagnostic_collects_scan_identity_for_bounded_attribution`.
- Existing plan-estimate coverage verifies the versioned source metadata used
  by the diagnostic.
- Workspace server compilation and strict Clippy pass.

## Boundary

The attribution is deliberately limited to scan nodes whose telemetry can be
matched one-for-one. Per-operator actual cardinality beyond scans, selectivity
attribution, and release-threshold automation remain open and are not implied
by this endpoint.
