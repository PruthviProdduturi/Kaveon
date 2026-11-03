# Live stage estimate metadata — 2026-11-03

## Scope

Distributed task completion now carries the planner's exact stage operator and
row estimate into the running query record. If a task published the stage
before that metadata was available, a later completion fills the metadata on
the existing stage without changing its measured task rows. The estimate is
never inferred from task output or the final result.

This makes the existing `/v1/query/{id}/estimates` diagnostics useful while a
distributed query is still running, rather than only after final stage
assembly.

## Evidence

The server unit
`a_running_record_reports_each_finished_task` publishes a stage once without
metadata, then publishes a completion with `Filter` and estimate `100`; the
running record exposes both values while retaining the exact task telemetry.

```text
cargo test -p kaveon-server --bin kaveon-server \
  a_running_record_reports_each_finished_task -- --nocapture
test result: ok. 1 passed; 0 failed

cargo clippy --workspace --all-targets -- -D warnings
Finished `dev` profile
```

## Boundary

This closes live stage metadata propagation only. It does not invent
intermediate operator row counts: filters, joins, aggregates, limits, and set
operations still require runtime counters at their own physical boundaries
before they can receive per-operator actual attribution.
