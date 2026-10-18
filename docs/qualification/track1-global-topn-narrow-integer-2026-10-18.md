# Global TopN narrow-integer qualification

## Scope

This qualification covers the bounded typed fast path used by global `ORDER BY … LIMIT` and distributed TopN merges. Integer keys from `TINYINT` through `BIGINT` (`Int8`/`Int16`/`Int32`/`Int64` and their unsigned Arrow equivalents) now use direct typed selection and k-way merge logic instead of the general Arrow lexsort path. The comparison remains exact for signed and unsigned extrema, and nullable keys continue through Arrow's null-aware path.

The change is intentionally local. It does not claim that the coordinator has a fully parallel ordered result stream; that remains open in the Track 1 tracker.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec topn --quiet
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The focused TopN suite passed 19/19 tests. The typed narrow-integer parity test compares `Int8`, `Int16`, `Int32`, `UInt8`, `UInt16`, and `UInt32` results with Arrow `lexsort_to_indices` for ascending and descending order and limits `0`, `1`, `3`, and larger than the input. Existing tests continue to cover nullable ordering, signed/unsigned extremes, ties, multi-key ordering, partition merging, and bounded operator memory.

## Boundary

This is local implementation evidence only. Cloud/AKS qualification, broader complex key types, and elimination of the coordinator bottleneck remain open.
