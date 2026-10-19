# Track 1 — Shared window sort context qualification

## Scope

When multiple window expressions share the same `PARTITION BY` and `ORDER BY` layout, the executor now caches the evaluated partition and ordering arrays together with the lexsort permutation. Each expression still evaluates its own frame and function, while key expressions are evaluated once per layout. Independent expressions continue to run concurrently and preserve declaration order in the output schema.

## Evidence

```text
cargo test -p kaveon-exec --lib window::tests --quiet
cargo fmt --all -- --check
```

The window unit suite passed 14/14 tests, including multiple expressions with distinct frames and partitioned ordered frames. The existing distributed window differential and planner qualification continue to cover the three-worker partitioned path.

## Boundary

This removes repeated key evaluation inside one window operator; it does not distribute a single partition across workers, and unpartitioned or mixed-partition window plans remain centralized for global ordering and frame correctness. Deployed cloud qualification remains open.