# T-SQL `STRING_AGG` boundary — 2026-10-30

## Status

`STRING_AGG` is **explicitly rejected** in the bounded T-SQL read contract.
This is a deliberate fail-closed boundary, not an unsupported-function leak.

## Why it remains open

Correct distributed execution requires a mergeable string state with defined
ordering, NULL handling, separator semantics, memory limits, spill behavior,
and cancellation. Kaveon's current aggregate exchange has no ordered string
state, so accepting the syntax would either produce non-deterministic output or
silently execute only a coordinator-local fragment.

## Evidence

- The versioned corpus contains the `string_agg` case with
  `status: explicit_rejection` and
  `distributed_status: requires_mergeable_ordered_string_state`.
- `cargo test -p kaveon-sql --lib rejects_string_agg_with_a_bounded_rewrite_hint`
  passes.
- The parser rejects the function by a word-boundary match, so identifiers such
  as `STRING_AGG_value` are not misclassified.
- The diagnostic names the missing contract and suggests
  `ARRAY_AGG` plus `ARRAY_JOIN` only where that bounded rewrite is semantically
  acceptable.

## Exit criteria

Move this case to qualified only after a mergeable distributed state is added,
with exact local/three-worker results, bounded-memory and spill tests,
cancellation coverage, and an explicit ordering contract for `WITHIN GROUP`.
