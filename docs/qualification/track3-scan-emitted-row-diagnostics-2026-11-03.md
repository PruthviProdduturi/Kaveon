# Track 3 — scan emitted-row diagnostics

## Contract

Finished query diagnostics now retain two distinct scan cardinalities:

- `actual_rows_selected`: rows in row groups selected by the reader; and
- `actual_rows_emitted`: rows emitted after decoder-side row filtering.

The distinction is exposed on physical scan plan nodes and in the per-scan
estimate diagnostics. Existing estimate ratios continue to use selected rows,
because source statistics describe the selected scan input; emitted rows are
reported as an additional, explicitly scoped output boundary. No rows are
inferred for non-scan operators.

## Evidence

- `annotate_actual_scans` attaches both counters from the finished scan
  telemetry.
- `actual_scan_rows_are_attached_only_when_scan_order_is_known` verifies the
  scan-node attributes and preserves scan-order matching.
- `operator_estimates_report_only_attached_runtime_rows` verifies emitted rows
  are carried alongside the selected-row estimate diagnostic.
- `cargo test -p kaveon-server actual_scan_rows_are_attached_only_when_scan_order_is_known -- --nocapture` — passed.
- `cargo test -p kaveon-server operator_estimates_report_only_attached_runtime_rows -- --nocapture` — passed.
- `cargo fmt --all -- --check` — passed.

## Boundary

This qualifies scan-level selected/emitted cardinality only. Filter, join,
aggregate, window, and sort operators still require native runtime counters
before they can receive per-operator actual rows; parent or child totals must
not be inferred as those operators' output.
