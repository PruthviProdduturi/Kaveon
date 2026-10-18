# Global TopN narrow-integer qualification

## Scope

This qualification covers the bounded typed fast path used by global `ORDER BY … LIMIT` and distributed TopN merges. Before this change, only `BIGINT`/`UBIGINT` (`Int64`/`UInt64`) keys avoided the general Arrow lexsort path. `INT`/`UINT` keys now use the same direct typed selection and k-way merge logic.

The change is intentionally local. It does not claim that the coordinator has a fully parallel ordered result stream; that remains open in the Track 1 tracker.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec topn --quiet
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The focused TopN suite passed 18/18 tests. The new parity test compares `Int32` and `UInt32` results with Arrow `lexsort_to_indices` for ascending and descending order and limits `0`, `1`, `3`, and larger than the input. Existing tests continue to cover nullable ordering, signed/unsigned extremes, ties, multi-key ordering, partition merging, and bounded operator memory.

## Boundary

This is local implementation evidence only. Cloud/AKS qualification, broader complex key types, and elimination of the coordinator bottleneck remain open.
