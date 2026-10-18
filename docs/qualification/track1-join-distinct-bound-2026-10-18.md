# Track 1 — Distinct subquery join costing

## Scope

A `SELECT DISTINCT` subquery can only reduce the input relation's cardinality. The optimizer now preserves the source row and byte upper bounds through that wrapper, allowing a proven small build side to use a safe broadcast join instead of falling back to partitioning. The bound remains conservative because distinct output is never estimated as larger than its input.

## Evidence

```text
cargo test -p kaveon-optim keeps_upper_bound_through_a_distinct_subquery -- --nocapture
result: 1 passed, 0 failed
```

The regression plans `big JOIN (SELECT DISTINCT id FROM small)` with exact source statistics and verifies `BroadcastRight`.

## Boundary

This qualifies planner costing for direct DISTINCT subqueries. It does not estimate the reduced distinct cardinality, model key skew, or replace live AKS mixed-workload qualification.
