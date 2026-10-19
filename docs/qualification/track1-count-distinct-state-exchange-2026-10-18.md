# Exact grouped COUNT DISTINCT state exchange qualification

## Scope

The in-process grouped aggregate state path now has an exact regression for
three-worker-style state exchange: duplicate values appearing in separate
worker states are unioned once, NULL values are excluded, and an empty worker
contributes no state without changing the result.

## Evidence

`grouped_count_distinct_exchange_unions_duplicates_nulls_and_empty_partitions`
creates two partial grouped states, round-trips each through the Arrow state
encoding used by exchange, merges them, and verifies the exact result of
three distinct non-NULL values. The existing
`memory_aware_aggregate_bounds_group_and_distinct_state` regression continues
to verify bounded admission and complete reservation release under pressure.
`hybrid_merge_spill_unions_duplicate_distinct_values_across_runs` additionally
forces the grouped final merge to write multiple spill runs while repeating
the same distinct value in separate partial states; replay returns exactly two
values per group and releases both memory and spill bytes.

## Boundary

This qualifies the existing in-process grouped state exchange and bounded
local final-merge spill/replay for exact distinct state. It does not enable
the API distributed aggregate shortcut for arbitrary exact `COUNT(DISTINCT)`
plans, and it does not claim broader topology, disk-failure, or deployed
cloud qualification.
