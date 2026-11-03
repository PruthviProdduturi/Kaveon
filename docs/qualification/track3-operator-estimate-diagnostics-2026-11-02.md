# Operator estimate diagnostics — 2026-11-02

## Scope

The Engine estimate-diagnostics endpoint now exposes `per_operator` entries
when a physical-plan node has both an estimated row count and an actual row
counter attached by the runtime. Each entry includes the operator, node ID,
estimate, actual rows, ratio, classification, estimate scope/source, and the
actual-row scope. The endpoint deliberately omits nodes without both values;
it never derives an intermediate count from a parent, child, or final result.

For distributed fragment execution, the endpoint also exposes
`stage_boundaries` when a fragment root has an exact operator name and bounded
estimate. The actual is the sum of completed task output rows for that stage,
labelled `stage_output_rows`; coordinator-only aggregate/TopN shortcuts
without a fragment-root estimate remain unavailable.

## Evidence

The server unit `operator_estimates_report_only_attached_runtime_rows` verifies
that a scan with `estimated_rows=100` and `actual_rows_selected=40` is emitted
as an `overestimated` operator diagnostic with a `0.40` ratio, while an
intermediate filter without an actual counter is omitted.

```text
cargo test -p kaveon-server operator_estimates_report_only_attached_runtime_rows \
  --all-targets --no-default-features -- --nocapture
test result: ok. 1 passed; 0 failed
```

The `stage_estimates_report_only_exact_fragment_root_boundaries` unit verifies
that a measured fragment root reports `40 / 100` as `overestimated`, while the
same stage without an exact operator identity produces no diagnostic.

## Boundary

Root, scan, and exact fragment-root stage output counters are qualified. Full
intermediate operator CPU, memory, spill, blocked-time, and selectivity
attribution remains open until those counters are emitted by the execution
runtime and qualified across distributed stages.
