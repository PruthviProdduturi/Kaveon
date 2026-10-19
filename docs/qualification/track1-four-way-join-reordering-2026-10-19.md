# Track 1 — four-way join reordering — 2026-10-19

## Scope

The statistics optimizer now enumerates connected left-deep orders for a
four-to-six-relation inner equi-join chain when every leaf has exact row and
byte facts. It chooses the bounded order with the smallest conservative
intermediate footprint, orients each equality condition for the chosen build
order, and adds an explicit projection so `SELECT *` retains the written
relation order.

The rewrite remains fail-closed for outer joins, `USING`, residual predicates,
duplicate aliases, missing statistics, and non-left-deep join graphs. It does
not claim a general join enumerator or cardinality-selectivity model.

## Evidence

The optimizer regressions
`statistics::tests::reorders_a_four_way_chain_with_exact_output_order` and
`statistics::tests::reorders_a_five_way_chain_with_a_connected_left_deep_order`
cover exact source bytes. They verify that the chosen plans contain the small
relations in the first stages and that projections expose the original written
column order. The existing three-way and outer/`USING` fail-closed tests
continue to pass.

```text
cargo test -p kaveon-optim --all-targets
cargo fmt --all -- --check
cargo clippy -p kaveon-optim --all-targets -- -D warnings
```

## Boundary

Seven or more relations, bushy plans, non-equality join predicates, and
selectivity/NDV-aware cost models remain open. Cloud and deployed qualification
still require a current Engine image and an available cluster.
