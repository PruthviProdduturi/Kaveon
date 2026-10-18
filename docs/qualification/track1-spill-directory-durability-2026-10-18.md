# Track 1 spill directory durability — 2026-10-18

## Boundary

Completed spill streams already sync their file contents before the partial
file is atomically renamed to its final run name. The writer now also syncs the
containing directory after that rename on Unix, making the published directory
entry durable before the run is returned to the execution pipeline. This
closes the crash window where the bytes exist but the final name is lost during
node or process recovery.

Windows keeps the existing atomic rename and file sync path because the standard
library does not expose portable directory fsync there.

This covers local spill publication durability only. It does not claim remote
object-store spill, replicated spill metadata, or recovery across hosts.

## Evidence

The existing atomic-publication and active-reader lifetime tests continue to
verify that partial streams are never exposed and completed runs remain readable
until their final reader is dropped:

```text
cargo test -p kaveon-exec spill -- --nocapture
```
