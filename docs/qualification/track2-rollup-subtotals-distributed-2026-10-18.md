# `ROLLUP` subtotal and grand-total qualification — 2026-10-18

The server differential suite now compares a bounded two-key `ROLLUP` through
the node-local planner and the three-worker fragment path. The fixture uses a
nullable `industry` dimension, so detail rows with a source `NULL` remain
distinct from the `NULL` introduced by the industry subtotal and the final
grand-total branch.

The query projects `GROUPING(region)`, `GROUPING(industry)`, and
`GROUPING_ID(region, industry)`. The regression checks exact local/distributed
row equality, the Trino masks `0`, `1`, and `3`, the nullable detail value, the
grand-total row, and zero query-memory residue after both executions.

This qualifies the bounded column-key `ROLLUP` lowering for subtotal and
grand-total rows. Arbitrary expression grouping keys, unbounded expansion,
and broader grouping-set combinations remain outside this evidence.
