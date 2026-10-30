# T-SQL `STRING_AGG` qualification — 2026-10-30

## Status

The bounded unordered form `STRING_AGG(value, literal_separator)` is planner
and executor qualified. It omits NULL values, returns NULL for an empty group,
and carries a mergeable string state through the distributed aggregate
exchange. Partial merge order is intentionally unspecified, matching SQL
Server's unordered form. Each group is bounded to a 16 MiB materialized
result; over-limit values fail closed instead of allocating without bound.

## Why it remains open

Ordered `WITHIN GROUP` forms, dynamic separators, `DISTINCT`, and `FILTER`
remain fail-closed. Correct support for those forms needs an ordered merge
state and additional bounded-memory/spill semantics; this qualification does
not claim them.

## Evidence

- The versioned corpus contains the `string_agg` case with
  `status: planner_qualified` and
  `distributed_status: qualified_for_unordered_literal_separator`.
- `cargo test -p kaveon-sql --lib` passes the planner acceptance and explicit
  rejection cases.
- `cargo test -p kaveon-exec --lib` passes the merge and Arrow-state round trip
  regression for NULL omission, literal separator validation, and partial
  concatenation.
- `cargo test -p kaveon-server --bin kaveon-server
  partial_and_final_string_agg_merge_literal_separator_states` passes the
  partial/final fragment path, including compact state serialization.
- `cargo clippy -p kaveon-sql -p kaveon-core -p kaveon-exec -p kaveon-server
  --all-targets -- -D warnings` passes.

## Exit criteria

The next qualification step is an exact local/three-worker query differential,
bounded-memory/spill and cancellation evidence for string state, followed by a
separate ordered `WITHIN GROUP` contract. Until then, only the unordered literal
separator form is advertised.
