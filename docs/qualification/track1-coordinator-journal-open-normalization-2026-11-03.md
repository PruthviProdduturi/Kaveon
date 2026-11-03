# Track 1 coordinator journal open normalization — 2026-11-03

## Boundary

Opening a coordinator journal can normalize an over-bound history or remove
malformed trailing records. That normalization now uses the same staged-file,
`sync_data`, atomic-rename, and Unix parent-directory sync sequence as normal
compaction. A restart therefore never has to truncate the acknowledged journal
in place before the replacement snapshot is durable.

This is local journal durability only. It does not claim a distributed lease,
multi-host journal replication, object-store journal recovery, or active-query
continuity across coordinator loss.

## Evidence

The bounded restart regression appends past the retention threshold, reopens
the journal, and verifies that the newest 1,000 records remain parseable and
ordered. The same test also exercises the malformed trailing-line recovery
path:

```text
cargo test -p kaveon-server --bin kaveon-server \
  the_query_log_outlives_the_process_and_stays_bounded -- --nocapture
```

Result on 2026-11-03: `1 passed; 0 failed`.
