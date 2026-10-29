# Plan estimate visibility qualification — 2026-10-29

## Scope

Optimized and physical `Scan` nodes now expose bounded planning facts in
`EXPLAIN` and query records when the source was resolved during planning:

- `estimated_rows` from the pinned source metadata;
- `estimated_bytes` when storage reports exact bytes;
- `estimate_source` (`current_statistics` or `source_metadata`);
- `estimate_source_version`, the source identity used by the planner.

The annotations are descriptive only. They do not alter execution, and they
do not claim that a scan estimate is the output cardinality after filters,
joins, limits, or aggregates.

## Evidence

- `api::tests::plan_scan_displays_versioned_source_estimates_without_rewriting_execution`
  verifies rows, bytes, source label, and source identity are attached without
  changing the plan operator.
- `cargo clippy -p kaveon-server --all-targets -- -D warnings` passes.
- The annotations are produced from the same pinned planning relations used by
  statistics-aware join ordering and source pruning.

## Remaining boundary

Operator-level estimated-vs-actual matching, selectivity error reporting, and
plan-aware release thresholds remain open. This record qualifies visibility of
the planner's source assumptions only.
