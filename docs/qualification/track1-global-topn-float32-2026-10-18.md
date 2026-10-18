# Track 1 — finite Float32 TopN qualification

## Scope

Global `ORDER BY ... LIMIT` candidate selection now uses a direct typed comparator for non-null finite `Float32` keys. This avoids the general Arrow lexsort materialization for the common finite path while preserving the existing fallback for NULL and NaN values, whose ordering semantics remain delegated to Arrow.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec topn --quiet
running 23 tests
23 passed; 0 failed

cargo fmt --all -- --check
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The regression compares ascending and descending finite Float32 results for limits 0, 1, 3, and above the input size against Arrow `lexsort_to_indices`; it also verifies NaN declines the typed path.

## Boundary

This is local/in-process TopN evidence. NULL and NaN Float32 keys intentionally retain Arrow's general path. Cloud/AKS qualification and a fully parallel ordered result stream remain open.