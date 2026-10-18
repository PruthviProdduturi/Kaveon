# Track 1 streaming exchange buffer — 2026-10-18

## Boundary

Streaming Arrow exchange output now starts its producer buffer at the
configured exchange chunk size. The common split-and-swap path still avoids
copying the tail when a batch crosses a boundary, while the initial buffer
does not need repeated growth for the first full chunk.

## Evidence

The focused server test verifies that a new `StreamingOutput` owns a buffer
with exactly the configured chunk capacity:

```text
cargo test -p kaveon-server exchange::tests::streaming_exchange_buffer_starts_at_one_chunk_capacity -- --nocapture
```

The existing streaming chunk-order, boundary, and Arrow round-trip tests
continue to cover the split-and-swap behavior.

## Remaining boundary

This reduces producer-side exchange allocation churn. It does not claim a
fully streaming coordinator result path or eliminate Arrow serialization cost.
