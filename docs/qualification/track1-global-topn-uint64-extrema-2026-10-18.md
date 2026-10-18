# Track 1 — distributed UInt64 TopN extrema qualification

## Scope

The global `ORDER BY ... LIMIT` merge preserves the full unsigned 64-bit ordering domain across worker-local candidates. This qualification covers `UInt64::MAX`, the adjacent maximum, ordinary values, and NULL placement across three worker batches. The distributed candidate result is compared with a single-partition Arrow lexsort reference.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec topn --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The focused test `three_worker_global_uint64_top_n_preserves_extrema` verifies the exact top five rows across three workers, including the two largest possible `UInt64` values and NULLS LAST.

## Boundary

This is local/in-process distributed merge evidence. Cloud/AKS qualification and a fully parallel ordered result stream remain open.