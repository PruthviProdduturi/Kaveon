# Track 1 — coordinator journal restart and takeover evidence

## Scope

The coordinator query journal preserves terminal history across process restart and remains parseable when a replacement coordinator opens the same state directory concurrently. Restored active records are marked as recovered history instead of being presented as live task state.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server a_restart_puts_the_query_history_back_in_the_store --quiet
running 1 test
.\ntest result: ok. 1 passed; 0 failed

cargo test -p kaveon-server concurrent_coordinator_journals_keep_every_terminal_record_parseable --quiet
running 1 test
.\ntest result: ok. 1 passed; 0 failed
```

The first regression covers restart restoration and replacement of an in-memory live record by journal state. The second covers two coordinators appending terminal records to the same bounded journal and reopening the result without malformed or duplicate records.

## Boundary

This is local durable-journal evidence. It does not claim multi-coordinator active-query continuity or distributed consensus; those remain outside the current runtime contract.