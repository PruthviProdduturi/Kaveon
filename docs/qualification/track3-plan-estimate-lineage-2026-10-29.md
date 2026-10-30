# Track 3 — physical-plan estimate lineage — 2026-10-29

## Boundary

Physical plan display now carries conservative source-estimate lineage beyond
scan nodes. Unary operators expose their known child input row count; joins
and semi/anti joins expose left and right input estimates; set operations
expose the sum of known child inputs. Each value is marked
`estimate_scope=child_input_only` (or `child_inputs_only`).

These fields are diagnostic metadata only. Kaveon does not present them as
post-filter, post-join, or post-aggregation output cardinalities, and they do
not alter execution or join selection.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server plan_estimates_expose_conservative_join_input_lineage -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
```

The focused test and workspace clippy gate pass on 2026-10-29. Exact
operator-output estimates and selectivity attribution remain a separate open
qualification boundary.
