# Track 1 final-merge shared-pressure qualification — 2026-10-18

## Boundary

Threaded final merges share one query memory budget with their partial aggregators. A refusal can otherwise arrive between a final merge batch and its retry, leaving no headroom for the spill path.

## Change

`HybridFinalMerge` now records the query limit and, only for threaded merges with a pressure responder and an available spill device, spills its live grouped table after the account crosses half the budget. Serial merges retain refusal-driven spilling. Existing reservations, spill accounting, and cleanup remain in force.

## Evidence

From the isolated integration worktree after commit `8042fdf8`:

- `cargo test -p kaveon-server a_distributed_grouped_aggregate_under_spill_matches_the_local_result -- --nocapture` — 1 passed.
- `cargo test -p kaveon-server parallel_final_with_a_top_n_tail_matches_the_serial_merge_and_spills_on_refusal -- --nocapture` — 1 passed.
- `cargo test -p kaveon-server hybrid_final_aggregate_keeps_typed_null_keys_through_a_spill_and_bounds_merge_memory -- --nocapture` — 1 passed.
- `cargo test -p kaveon-server fragment_exec::tests -- --nocapture` — 21 passed, 0 failed.
- `cargo test -p kaveon-exec final_merge -- --nocapture` — 7 passed, 1 ignored.

This qualifies the shared-pressure behavior for the current local test topology. Distributed object-store and deployed endurance qualification remain separate gates.
