# Runtime regression threshold qualification — 2026-10-29

## Scope

`GET /v1/query/{query_id}/comparison` compares a completed statement with
successful, non-cache executions of the same normalized SQL in the same
catalog and schema.

The endpoint now exposes three bounded fields after at least five comparable
runs:

- `regression_threshold_ms`: two times the observed P95 duration;
- `regression_ratio`: current duration divided by P95;
- `regression_detected`: true only when current duration is strictly above
  the doubled-P95 threshold.

With fewer than five runs all three fields are omitted (`null`). This avoids
turning a small sample into a performance claim. Cache hits remain excluded
from the baseline, and the signal is runtime-only; it is not an estimate-vs-
actual plan diagnostic.

## Evidence

- `api::tests::runtime_regression_requires_five_runs_and_uses_a_doubled_p95`
  verifies the minimum sample, threshold, ratio, and boundary behavior.
- `cargo clippy -p kaveon-server --all-targets -- -D warnings` passes.
- API contract is documented in `docs/reference/api.md`.

## Remaining boundary

Estimate-vs-actual operator diagnostics and plan-aware release thresholds are
still open. This record qualifies only the conservative runtime signal.
