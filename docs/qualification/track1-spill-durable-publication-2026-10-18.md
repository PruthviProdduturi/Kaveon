# Track 1 qualification: durable spill publication

## Scope

Spill runs now call `sync_data` on the completed Arrow IPC stream before the
temporary `.partial` path is atomically renamed to its published `.arrow`
path. The rename prevents readers from observing an incomplete stream, while
the sync makes the bytes durable before the run becomes discoverable after a
process or node failure.

## Validation

The existing atomic-publication regression verifies that a run remains hidden
while its stream is incomplete, is published only after `finish`, and is
removed with its reservation when the final run handle is dropped:

```text
cargo test -p kaveon-exec spill::tests::unfinished_run_is_not_published_until_stream_is_complete -- --nocapture
cargo test -p kaveon-exec spill::tests::streaming_reader_keeps_run_alive_after_owner_drops -- --nocapture
```

Both checks pass on 2026-10-18. This qualifies local filesystem publication
ordering and durability calls; it does not qualify object-store spill or a
node-loss fault injection.

