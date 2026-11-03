# Qualification: view-backed text keys in distributed aggregate routing

Date: 2026-11-03  
Scope: distributed final aggregate selection (`kaveon-server`)

## Contract

A grouped aggregate whose key is Arrow `Utf8View` is a text-key workload. The
distributed final-aggregate planner must select the text partitioning path,
not the integer/broadcast fallback, so source-thread partials are routed once
to their owning worker. This is a routing decision only; it does not change
the logical result type, which remains ordinary UTF-8 text.

## Evidence

- `cargo test -p kaveon-server --bin kaveon-server utf8_view_group_keys_use_the_text_partitioning_path`
  — passes for `Utf8View`, `LargeUtf8`, and a non-text control type.
- `cargo test -p kaveon-server --bin kaveon-server parallel_final_partitioned_text_keys_are_not_selected_twice`
  — passes; verifies the existing multi-source text routing result is not
  double-selected or dropped.
- `cargo clippy -p kaveon-server --bin kaveon-server --all-targets -- -D warnings`
  — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies the text-key routing decision. It does not claim broader
high-cardinality adaptive join/aggregate completion, which remains a separate
Phase 4 gate.
