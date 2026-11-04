# Operator estimate scope preservation — 2026-11-03

## Scope

The estimate-diagnostics collector now preserves the runtime scope attached to
an actual output counter. A result propagated through a `Window`/`Project`/
`Sort` chain is reported as `derived_row_preserving`; a root result remains
`query_result`, and scan counters remain `scan_selected_rows`. The endpoint no
longer relabels every `actual_output_rows` value as a final query result.

This is attribution metadata only. It does not infer selectivity or invent
intermediate rows, and it does not change planning or execution.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server \
  operator_estimates_preserve_derived_row_preserving_scope -- --nocapture
test result: ok. 1 passed; 0 failed

cargo test -p kaveon-server --bin kaveon-server \
  operator_estimates_report_only_attached_runtime_rows -- --nocapture
test result: ok. 1 passed; 0 failed
```

The first test verifies a derived row-preserving `Project` counter retains its
scope and ratio. The second preserves the existing fail-closed scan boundary
coverage, including scan selectivity metadata.

## Boundary

Intermediate operator counters, CPU/memory/spill attribution, and distributed
stage resource diagnostics remain open until the runtime emits those counters
and they are qualified across the relevant execution paths.
