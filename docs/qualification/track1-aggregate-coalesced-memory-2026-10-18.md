# Track 1 — coalesced partial memory accounting — 2026-10-18

## Scope

The optional high-cardinality pass-through coalescer now retains query memory reservations for each encoded batch while those batches are held for concatenation. The reservations are released after the concatenated result is built, so a multi-batch coalescing window cannot bypass the aggregate memory budget between partial flushes.

## Evidence

Command:

```text
cargo test -p kaveon-exec coalesced_passthrough_keeps_retained_batches_memory_accounted -- --nocapture
```

Result: 1/1 passed. The regression forces a two-row coalescing window over one-row source batches, verifies the output is coalesced, keeps peak usage below the 4 MiB query limit, and drains all reservations after the operator and result are released.

## Boundary

This covers the opt-in `KAVEON_COALESCE_PARTIAL_ROWS` pass-through path. Normal one-batch pass-through and disk spill accounting are unchanged; the guard covers retained encoded Arrow batches during concatenation.
