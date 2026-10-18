# Track 1 — aggregate zero-copy selected-row merge

Partitioned final aggregate workers now enable the columnar selected-row merge
path by default. The merge receives hash-selected row indices directly,
avoiding an intermediate Arrow `filter`/`take` batch allocation. Aggregate
layouts that cannot consume selected indices still use the existing filtered
fallback inside `push_selected`, preserving the compatibility boundary.

## Evidence

```text
cargo test -p kaveon-exec --lib final_merge --quiet
test result: ok

cargo clippy -p kaveon-exec --all-targets -- -D warnings
Finished successfully

cargo fmt --all -- --check
passed
```

The final-merge unit corpus exercises exact grouped output with partition
selection, pressure-triggered spill, source refusal, and bounded replay. This
change enables the already-tested selected-row implementation for the production
partitioned path; cloud deployment qualification and physical disk exhaustion
remain open.
