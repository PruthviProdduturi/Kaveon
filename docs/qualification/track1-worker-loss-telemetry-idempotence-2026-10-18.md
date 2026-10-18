# Worker-loss telemetry idempotence qualification — 2026-10-18

The coordinator records one current telemetry row per stage partition. If a
worker loss causes a partition to be retried, the recovery record retains the
full retry history while the stage telemetry replaces the earlier partition
row. This prevents scan totals and completed-task counts from double-counting
rows that were produced by an obsolete attempt.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server --bin kaveon-server \
  api::streamed_root_tests::a_worker_lost_between_the_producers_and_the_consumers_is_recovered_exactly \
  -- --nocapture
```

Result: **1 passed**. The recovery record still reports the lost worker and
three retry attempts, while the final stage telemetry contains exactly one
row for each of the two scan partitions (`0`, `1`).

This is local/in-process evidence. It does not qualify AKS node-loss
endurance or a cloud object-store failure run.
