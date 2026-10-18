# Track 1 — atomic spill-run publication qualification

## Scope

Spill writers now write to a `.arrow.partial` path and atomically rename it to
the finished `.arrow` path only after the Arrow IPC stream has been flushed.
Readers therefore cannot open a partially written stream as a completed run.
An unfinished writer still removes its partial file and releases its reserved
bytes when dropped.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec spill::tests:: -- --nocapture
test result: ok; 11 passed; 0 failed
```

The focused regression verifies that a live writer exposes only the partial
marker, publishes the final path after `finish`, and removes the finished run
when its owner is dropped. Existing quota, corruption, concurrent-admission,
schema-mismatch, and reader-lifetime tests also pass.

## Boundary

This covers atomic in-process publication and cleanup after an interrupted
writer. It does not claim process restart cleanup, object-store spill
durability, or node-loss recovery.
