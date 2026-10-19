# Track 1 — Mixed-partition window planning qualification

## Scope

Window expressions with different `PARTITION BY` keys cannot share one hash exchange. The planner must gather them into one final window stage so each expression sees its complete logical partition instead of producing worker-local partial frames.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server planner::tests::mixed_partition_windows_remain_gathered_for_correct_frames --quiet
cargo test -p kaveon-server --bin kaveon-server differential_tests::distributed_mixed_partitioned_windows_match_local_reference --quiet
cargo fmt --package kaveon-server
cargo clippy -p kaveon-server --all-targets -- -D warnings
```

The planner regression plans `ROW_NUMBER() OVER (PARTITION BY region ...)` beside
`ROW_NUMBER() OVER (PARTITION BY country ...)` and requires one `FinalWindow`
task with one input exchange. The three-worker differential then compares that
centralized distributed plan with the local reference on the same mixed-key
query, including independent order tie-breakers. Same-key window expressions
remain eligible for the partitioned path.

## Boundary

This closes the mixed-partition planning guard locally. It does not claim distributed execution for mixed keys; that requires multiple exchange outputs and a row-preserving merge design.
