# Track 1 — spill reader lifetime qualification

## Scope

`SpillRunReader` now retains the underlying `SpillRun` ownership while it is
open. Dropping the caller's `SpillRun` handle during streaming no longer
removes the file underneath the reader or releases its byte reservation before
the reader reaches EOF. The file and accounting are released after the reader
is dropped, preserving bounded spill cleanup.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec spill::tests:: -- --nocapture
test result: ok; 10 passed; 0 failed
```

The focused regression opens a reader, drops its owner, reads the complete
Arrow stream, and verifies that the file remains present during the read and
is removed with zero retained bytes after the reader closes. Existing quota,
corrupt-stream, concurrent-admission, schema-mismatch, and incremental-reader
tests also pass.

## Boundary

This covers in-process spill-run ownership and cleanup. It does not qualify
process crash recovery, object-store spill durability, or node-loss recovery.
