# Track 2 partitioned value-window qualification — 2026-10-18

The bounded distributed window path now has one exact local versus three-worker
differential for `FIRST_VALUE`, `LAST_VALUE`, and `NTH_VALUE`. The statement
uses `PARTITION BY region`, deterministic multi-column ordering, explicit
`ROWS` frames, nullable `industry` values, `IGNORE NULLS`, and a bounded
result. Each worker receives complete hash partitions before the window
operator evaluates frame-local values, so NULL skipping and frame boundaries
are not computed independently on arbitrary input fragments.

Focused gate:

```text
cargo test -p kaveon-server distributed_partitioned_value_windows_ignore_nulls_match_local -- --nocapture
test result: ok. 1 passed; 0 failed
```

The local planner and in-process three-worker fragment path return identical
canonical rows and release the query memory reservation. This qualifies the
bounded partitioned value-window shape only. Global unpartitioned value
windows remain on the centralized final stage; broader frame modifiers,
external-client qualification, and deployed cloud endurance remain open.
