# Track 3 — scan emitted-row diagnostics

## Contract

Finished query diagnostics now retain two distinct scan cardinalities:

- `actual_rows_selected`: rows in row groups selected by the reader; and
- `actual_rows_emitted`: rows emitted after decoder-side row filtering.
- `actual_filter_rows_examined` and `actual_filter_rows_admitted`: exact
  decoder predicate input/output counts when row filtering ran.
- `filter_selectivity`: `admitted / examined` when the examined count is
  nonzero; it is omitted when no filter count is available.

The distinction is exposed on physical scan plan nodes and in the per-scan
estimate diagnostics. Existing estimate ratios continue to use selected rows,
because source statistics describe the selected scan input; emitted and filter
counts are reported as additional, explicitly scoped boundaries. No rows are
inferred for non-scan operators.

## Evidence

- `annotate_actual_scans` attaches the selected, emitted, and filter counters
  from the finished scan telemetry.
- `actual_scan_rows_are_attached_only_when_scan_order_is_known` verifies the
  scan-node attributes and preserves scan-order matching.
- `operator_estimates_report_only_attached_runtime_rows` verifies emitted rows
  and filter counts are carried alongside the selected-row estimate diagnostic.
- `filter_selectivity_requires_examined_rows` verifies the derived ratio is
  omitted when no rows were examined.
- `cargo test -p kaveon-server actual_scan_rows_are_attached_only_when_scan_order_is_known -- --nocapture` — passed.
- `cargo test -p kaveon-server operator_estimates_report_only_attached_runtime_rows -- --nocapture` — passed.
- `cargo fmt --all -- --check` — passed.

## Boundary

This qualifies scan-level selected/emitted/filter cardinality only. Join,
aggregate, window, and sort operators still require native runtime counters
before they can receive per-operator actual rows; parent or child totals must
not be inferred as those operators' output.
