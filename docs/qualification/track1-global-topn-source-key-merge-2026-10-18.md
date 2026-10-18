# Track 1 — Bounded global TopN key merge

## Scope

The sorted-batch global TopN merge now compares Arrow ordering keys in their source batches. It avoids concatenating every worker key column into coordinator-wide temporary arrays before the bounded k-way heap merge, while preserving direction, NULL placement, multi-key ordering, and exact selected rows.

## Evidence

```text
cargo test -p kaveon-exec topn::tests -- --nocapture
result: 23 passed, 0 failed
cargo check -p kaveon-exec
cargo fmt --package kaveon-exec -- --check
```

The existing exact-result corpus covers string, decimal, float, unsigned integer, multi-key, NULL ordering, three-worker reference comparison, incremental candidate merging, and bounded candidate memory.

## Boundary

This reduces coordinator-side key-copy work for bounded TopN merges. It does not remove the coordinator's final result responsibility or provide a fully streamed ordered result protocol.
