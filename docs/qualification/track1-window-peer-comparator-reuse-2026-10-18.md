# Track 1 — partitioned window peer comparator reuse

## Scope

Partitioned window evaluation now builds each partition-key comparator once per input batch and reuses it while grouping adjacent rows into peer partitions. The previous path rebuilt an Arrow comparator for every adjacent peer comparison, adding repeated setup cost to distributed window fragments after exchange. Null placement and dictionary normalization remain unchanged; ordering/frame comparisons retain their existing semantics.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec window --quiet
running 15 tests
15 passed; 0 failed

cargo fmt --all -- --check
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The existing window suite continues to cover partition grouping, dictionary values, NULL peers, frames, ranking, and value-window behavior.

## Boundary

This is a local executor optimization with exact regression coverage. It does not distribute unpartitioned or mixed-key windows, and deployed cloud qualification remains open.