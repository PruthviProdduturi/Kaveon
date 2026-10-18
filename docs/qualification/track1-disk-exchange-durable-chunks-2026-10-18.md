# Track 1 qualification: durable disk exchange chunks

## Scope

The coordinator disk exchange store now writes and `sync_data`s each encoded
chunk before inserting it into the in-memory exchange index. A restart or
worker-loss replay therefore cannot observe an indexed chunk whose bytes were
only buffered in the operating system cache. Failed writes still release the
file and both global and per-query quota reservations.

## Validation

```text
cargo test -p kaveon-server disk_exchange::tests -- --nocapture
```

The disk exchange test suite passes on 2026-10-18, including stale-directory
reconciliation, reclaim-on-start, partial-attempt cleanup, failed-write quota
refunds, and coordinator teardown. This is local filesystem evidence; it does
not qualify a remote cloud object store or node-loss injection.

