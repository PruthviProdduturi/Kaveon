# Partitioned ordered window frame qualification

Date: 2026-10-18  
Scope: local differential harness; three in-process workers; Parquet dictionary,
plain, multi-file, Hive-partitioned, and clustered layouts.

## Shape qualified

The planner sends a window stage through one canonical hash exchange when all
window expressions use the same direct `PARTITION BY` column set. This run
adds an exact-result case with two expressions sharing `PARTITION BY region`:

```sql
SELECT country, region, actions,
       ROW_NUMBER() OVER (
         PARTITION BY region ORDER BY actions DESC, country
       ) AS rn,
       SUM(actions) OVER (
         PARTITION BY region ORDER BY actions DESC, country
         ROWS BETWEEN 2 PRECEDING AND CURRENT ROW
       ) AS rolling_actions
FROM events
ORDER BY region, rn, country
LIMIT 12
```

The stage graph uses one hash exchange on `region` and four final tasks. Each
worker therefore receives complete window partitions before evaluating the
ordered frame. Local and distributed canonical rows match exactly for every
layout in the differential corpus, and the query memory pool returns to zero.

## Validation

```text
cargo test -p kaveon-server --bin kaveon-server \
  planner::tests::partitions_ordered_frames_when_window_expressions_share_keys --quiet
  1 passed

cargo test -p kaveon-server --bin kaveon-server \
  differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
  1 passed; 363.74s

cargo fmt --all -- --check
git diff --check
```

This does not qualify unpartitioned windows, windows with different partition
key sets, arbitrary expression partition keys, cloud deployment behavior, or
long-running distributed-window pressure. Those remain open in the parity
tracker.
