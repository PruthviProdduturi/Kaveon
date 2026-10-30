# Track 1 coordinator journal compaction durability — 2026-10-18

## Boundary

When a coordinator compacts the bounded query journal, it now writes the
replacement JSONL file, calls `sync_data`, and only then publishes it through
the atomic rename. On Unix filesystems it also syncs the parent directory after
the rename, so the replacement name is durable rather than merely visible to
the current process. The existing interprocess lock still fences concurrent
coordinators across the append and compaction cycle.

This closes the crash window in which a successful compaction rename could
publish bytes that had not yet reached stable storage, including the directory
entry on Unix. Windows keeps the file-level sync guarantee because it has no
portable directory-fsync API in the supported standard library. It does not
claim a distributed lease, multi-host journal replication, or object-store
failure recovery.

## Evidence

The existing bounded restart test exercises repeated compaction and reopening,
and the concurrent-coordinator test exercises two journal handles writing
under the shared interprocess lock:

```text
cargo test -p kaveon-server --bin kaveon-server journal -- --nocapture
```

The tests preserve the bounded newest-record set and parse every retained
terminal record after concurrent replacement-coordinator writes.
