# Exact grouped `COUNT(DISTINCT)` API merge boundary

## Result

The API's scalar distributed aggregate shortcut remains fail-closed for exact
grouped `COUNT(DISTINCT)`. The planner regression
`keeps_exact_count_distinct_out_of_json_scalar_merge_shortcut` proves that a
query such as `SELECT region, COUNT(DISTINCT user_id) ... GROUP BY region`
does not enter `aggregate_merge_contract`.

## Why enablement is not safe yet

That shortcut sends each worker's finalized `TaskResponse` rows as JSON
scalars, then merges values with `MergeOperation::Add`, `Min`, or `Max`.
Adding worker-local distinct counts is incorrect when the same value occurs in
multiple workers. The qualified executor state path already exchanges the
typed distinct set and unions it exactly, but the API shortcut has no wire
representation for that set and no merge operation for it.

The exact missing contract is a bounded, versioned worker response carrying
typed `CountDistinct` state, including NULL exclusion, duplicate union, empty
partitions, and memory reservations during coordinator merge. Until that wire
contract exists, the planner must continue routing exact distinct aggregates
through the stateful execution path.

## Boundary

This is a planner/API safety qualification. It does not change eligibility or
claim disk-spill qualification for distinct state. The in-process state
exchange and local bounded-memory evidence remain in
`track1-count-distinct-state-exchange-2026-10-18.md`.
