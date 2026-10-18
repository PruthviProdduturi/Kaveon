# Track 1 qualification: durable query-journal append

## Scope

Terminal query records are now written, flushed, and synced before the
coordinator considers the journal append complete. A replacement coordinator
opening the same journal therefore sees complete terminal records after a
process restart, including records written immediately before failure.

## Validation

The journal bounds, concurrent-coordinator, reopen, and restoration tests cover
the same append path:

```text
cargo test -p kaveon-server the_query_log_outlives_the_process_and_stays_bounded -- --nocapture
cargo test -p kaveon-server concurrent_coordinator_journals_keep_every_terminal_record_parseable -- --nocapture
```

The query-journal test suite passes on 2026-10-18. This qualifies durable
local journal append and interprocess locking; it does not provide distributed
leader election or cross-host journal replication.

