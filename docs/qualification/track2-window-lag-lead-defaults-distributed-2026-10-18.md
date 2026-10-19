# Track 2 partitioned `LAG`/`LEAD` defaults qualification — 2026-10-18

The bounded partitioned window path now has an exact local versus three-worker
differential for `LAG` and `LEAD` with literal defaults and NULL treatment.
The statement uses a hash-partitioned `PARTITION BY region`, deterministic
multi-column ordering with `NULLS LAST`, nullable values, explicit `ROWS`
frames, and both positional `RESPECT NULLS` and `IGNORE NULLS` behavior.

Focused gate:

```text
cargo test -p kaveon-server distributed_partitioned_lag_lead_defaults_match_local -- --nocapture
test result: ok. 1 passed; 0 failed
```

The local planner and in-process three-worker fragment path return identical
canonical rows, including the literal default when the requested offset has no
qualifying value, and release the query memory reservation. The frame clause is
accepted as part of the window specification; offset functions retain their
positional semantics as in Trino. Global unpartitioned offset windows remain
on the centralized final stage. External-client, deployed-cloud, and broader
modifier qualification remain open.
