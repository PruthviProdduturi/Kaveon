# Track 1 Float32 runtime dynamic filtering — 2026-10-18

## Boundary

Join key extraction now preserves `REAL`/Arrow `Float32` identity in the
runtime filter path, including canonical `-0.0` and NaN handling. Previously
Float32 keys fell through the generic key path as unsupported, preventing the
partitioned join's build-side filter from being applied. The exact join index
and the runtime filter now use the same Float32 bit representation, so values
near the 24-bit precision boundary cannot be widened and compared incorrectly.

## Evidence

The focused executor regression verifies absent keys are rejected while values
above the Float32 integer precision boundary retain exact Float32 semantics:

```text
cargo test -p kaveon-exec runtime_join_filter -- --nocapture
```

The three-worker local/distributed differential uses `REAL` join keys and
compares the complete ordered result:

```text
cargo test -p kaveon-server distributed_float32_join_runtime_filter -- --nocapture
```

Both focused gates pass locally.

This covers Float32 runtime filtering only. Broader adaptive filter exchange
and cloud object-store qualification remain open.
