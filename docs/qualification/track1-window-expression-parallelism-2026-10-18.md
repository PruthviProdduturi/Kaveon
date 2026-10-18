# Track 1 unpartitioned window expression parallelism — 2026-10-18

## Boundary

The window operator still performs one shared global sort for an unpartitioned
window, preserving the required global ordering. When a statement contains
multiple independent window expressions, their frame evaluation now runs in
parallel over that shared immutable batch and sort permutation. Results are
joined back in declaration order, so the output schema and serialization stay
deterministic while CPU work can use more than one local thread.

This does not claim that the global sort or the distributed unpartitioned
window exchange is parallelized. Those remain centralized correctness paths and
are still open for a larger distributed window design.

## Evidence

The executor regression covers multiple expressions with different frames:

```text
cargo test -p kaveon-exec window -- --nocapture
```

The three-worker differential remains exact for global row ordering, rank peer
ties, and value windows with NULL skipping/defaults:

```text
cargo test -p kaveon-server distributed_unpartitioned_ -- --nocapture
```

All 16 executor window tests and all 4 matching distributed differentials pass
locally.
