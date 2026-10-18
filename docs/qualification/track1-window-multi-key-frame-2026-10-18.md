# Track 1/2 multi-key partitioned window qualification

Date: 2026-10-18
Scope: local differential harness; three in-process workers; dictionary, plain,
multi-file, Hive-partitioned, and clustered Parquet layouts.

## Shape qualified

The distributed planner can share one hash exchange for window expressions whose
partition keys are the same set in a different order. The exchange key list is
canonicalized before it is attached to the stage, while each worker evaluates
the original window ordering and frame locally after receiving complete logical
partitions:

```sql
SELECT country, region, platform, actions,
       ROW_NUMBER() OVER (
         PARTITION BY region, platform
         ORDER BY actions DESC, country
       ) AS rn,
       SUM(actions) OVER (
         PARTITION BY platform, region
         ORDER BY actions DESC, country
         ROWS BETWEEN 1 PRECEDING AND CURRENT ROW
       ) AS rolling_actions
FROM events
WHERE event_date = '2026-07-04'
ORDER BY region, platform, rn, country
LIMIT 16
```

The two expressions deliberately reverse the partition-key order. They are
still the same SQL partition relation, so one canonical hash exchange is safe.
The local and distributed results match exactly for every layout in the
differential corpus, including dictionary encoded text and the multi-file and
Hive-partitioned readers. The query memory pool returns to zero after each
distributed run.

## Validation

```text
KAVEON_DIFF_ONLY=partitioned_window_multi_key_frames \
cargo test -p kaveon-server --bin kaveon-server \
  differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths \
  -- --nocapture

cargo test -p kaveon-server --bin kaveon-server \
  planner::tests::shares_exchange_for_reordered_window_partition_keys --quiet

cargo fmt --all -- --check
git diff --check
```

This is a bounded multi-key qualification. Unpartitioned windows, mixed
partition-key sets, arbitrary expression keys, cloud deployment behavior, and
long-running distributed-window pressure remain open in the parity tracker.
