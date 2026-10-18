# Track 1 coordinator journal compaction durability — 2026-10-18

## Boundary

When a coordinator compacts the bounded query journal, it now writes the
replacement JSONL file, calls `sync_data`, and only then publishes it through
the atomic rename. The existing interprocess lock still fences concurrent
coordinators across the append and compaction cycle.

This closes the crash window in which a successful compaction rename could
publish bytes that had not yet reached stable storage. It does not claim a
distributed lease or multi-host journal replication.

## Evidence

The existing bounded restart test exercises repeated compaction and reopening,
and the concurrent-coordinator test exercises two journal handles writing
under the shared interprocess lock:

```text
cargo test -p kaveon-server query_journal -- --nocapture
```

The tests preserve the bounded newest-record set and parse every retained
terminal record after concurrent replacement-coordinator writes.
