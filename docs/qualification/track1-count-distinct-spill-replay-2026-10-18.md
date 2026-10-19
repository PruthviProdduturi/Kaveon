# Track 1 — exact grouped `COUNT(DISTINCT)` spill/replay

## Scope

Grouped exact distinct state remains typed state through the final merge when
memory pressure forces the merge table to spill. The replay path must union
values from separate partial states, so a duplicate appearing in different
spill runs is counted once.

## Evidence

`engine/crates/exec/src/final_merge.rs` test
`hybrid_merge_spill_unions_duplicate_distinct_values_across_runs` creates
12,000 groups with three partial states per group (`0, 1, 0`), splits those
states across batches, and runs the bounded eight-partition final merge. The
test observes multiple spill tables, exactly 12,000 groups, exactly two
distinct values per group (24,000 total), zero retained spill bytes, and zero
retained query-memory reservations.

The server fragment regression
`partial_and_final_aggregates_preserve_weighted_and_exact_state` and the
three-worker differential case `count_distinct_grouped_lowcard` cover the
same typed state contract through partial/final exchange and the in-process
three-worker fragment path.

## Boundary

This qualifies bounded local spill/replay and in-process three-worker exact
grouped distinct semantics. It does not claim physical disk exhaustion,
worker-loss recovery during distinct spill, broader topology, or deployed
cloud endurance. Exact plans also remain outside the scalar JSON merge
shortcut; they use typed fragment state.
