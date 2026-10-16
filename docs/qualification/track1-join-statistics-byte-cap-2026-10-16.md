# Track 1 join statistics and broadcast byte cap

This qualification covers the optimizer boundary added for source statistics:
Parquet, Delta, and local Iceberg snapshots expose exact row counts, schema
columns, and stored-byte estimates to join planning. A build side is broadcast
only when both its row estimate and its byte estimate are within the planner's
limits; an unknown byte estimate remains conservative.

## Reproduction

From `engine/` on the pinned Rust toolchain (`1.98.1`):

```text
cargo test -p kaveon-optim statistics --quiet
```

Result for commit `bf396e72`:

```text
13 passed; 0 failed
```

The focused tests cover:

- exact Iceberg snapshot schema and row-count extraction;
- row-count and stored-byte estimates from Parquet, Delta, and Iceberg paths;
- preservation of estimates through projections, sorting, limits, and offsets;
- broadcast selection for a materially smaller build;
- refusal of a build whose exact source bytes exceed `256 MiB`;
- right-side broadcast for a left outer join when the preserved left side
  remains the probe;
- preservation of join output ordering and `USING` metadata when a join is not
  safely reordered.

The release gate also ran `cargo clippy --workspace --all-targets -- -D
warnings`, the workspace tests, Linux/macOS/Windows release builds, and the
CLI qualification workflows successfully in GitHub Actions run
`38966141218`.

## Boundary

This is planner qualification, not a claim of complete cost-based optimization.
General multi-join enumeration, skew splitting, physical disk-exhaustion
qualification, and sustained mixed-workload AKS evidence remain open in the
parity tracker.
