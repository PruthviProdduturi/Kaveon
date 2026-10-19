# Three-way join reordering qualification

Date: 2026-10-18

The optimizer now enumerates the two connected starting pairs in a left-deep
three-relation inner equi-join chain. It uses exact source row counts and
stored bytes when both are available. The rewrite is bounded to three
relations and fails closed for outer joins, `USING`, residual predicates,
self-joins, missing statistics, and larger join graphs.

The selected plan keeps the original relation output order with an explicit
projection. Each generated equality is oriented to the physical left and
right inputs, so partitioned and broadcast joins continue to resolve their
keys against the correct schemas.

Evidence:

- `reorders_a_three_way_chain_to_reduce_the_first_stage_footprint`
  selects the smaller `medium JOIN tiny` stage before attaching `large` and
  asserts the final physical relation shape.
- `three_way_reordering_is_fail_closed_for_outer_and_using_joins` verifies
  that unsupported join semantics do not enter this rewrite.
- `cargo test -p kaveon-optim statistics` passed with 21 tests.

This is optimizer qualification for local logical plans. It does not claim
cloud, federation, arbitrary join-graph, or broad distributed endurance
qualification.
