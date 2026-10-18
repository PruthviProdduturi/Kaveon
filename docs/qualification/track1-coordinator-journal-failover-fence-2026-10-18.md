# Track 1 coordinator journal failover fence — 2026-10-18

## Boundary

An already-running coordinator can now survive a peer's bounded journal
compaction. Each append reopens the current journal path while holding the
existing interprocess lock, writes and syncs that inode, then replaces the
coordinator's cached handle. This prevents a stale pre-compaction file handle
from accepting a takeover or terminal record that disappears after restart.

This is a shared-filesystem stale-handle fence. It does not claim distributed
leader election, a lease service, journal replication, or multi-host object
store durability.

## Evidence

The focused regression opens two coordinator journal handles, forces compaction
through one handle, appends through the surviving handle, and verifies the
takeover record after reopening:

```text
cargo test -p kaveon-server a_surviving_coordinator_reopens_after_peer_compaction -- --nocapture
cargo test -p kaveon-server concurrent_coordinator_journals_keep_every_terminal_record_parseable -- --nocapture
```

Both tests pass locally. The existing concurrent-journal gate remains green,
including JSONL parsing and terminal-record retention under the shared lock.
