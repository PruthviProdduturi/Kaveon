# Optimizer local qualification — 2026-10-29

## Scope

This qualification records the current local optimizer boundary. It is not a
claim of cloud or AKS endurance, and it does not close the distributed-runtime
qualification items in the parity tracker.

## Evidence

Run from `engine/`:

```text
cargo test -p kaveon-optim --lib -- --nocapture
```

Result: **86 passed, 0 failed, 0 ignored** (14.95 seconds).

The suite covers statistics-aware build-side selection, exact source
byte/cardinality bounds, safe broadcast admission for filtered/limited/
distinct/projected/sorted relations, bounded four-to-eight-way connected
left-deep reordering with written projection order restored, three-way cost
reduction, filter propagation, scan pruning, correlated subquery
decorrelation, aggregate/cube planning, and fail-closed outer/USING/residual/
unknown/ambiguous join shapes.

## Boundary

The optimizer still qualifies only the bounded local planning contract above.
AKS disk-exhaustion, sustained mixed-workload, broader topology, adaptive
filter exchange, and cloud/deployed endurance evidence remain open. The
planner must continue to fail closed when exact relation facts or a supported
join shape are unavailable.

## Reproducibility

The workspace strict-lint gate also passed on the same tree:

```text
cargo clippy --workspace --all-targets -- -D warnings
```

No source or signature changes are implied by this document.
