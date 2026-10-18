# Track 1 qualification: catalog snapshot continuity during recovery

## Scope

The coordinator keeps a statement pinned to its catalog snapshot when choosing
replacement workers. If a worker converges to a newer snapshot while the
statement is still running, it is excluded rather than mixed into the replay.
The coordinator reports insufficient compatible workers until another worker
with the pinned snapshot is available.

## Exact local result

The scheduler regression starts with two workers on `snapshot-required`, moves
one to `snapshot-new`, and verifies that the old snapshot cannot continue with
only one compatible worker. It also verifies the cluster reports one worker
for each snapshot, proving the drift is explicit and does not silently mix
catalog versions.

```text
cargo test -p kaveon-server scheduler_rejects_snapshot_drift_during_recovery -- --nocapture
```

The regression passes on 2026-10-18. This is coordinator-level evidence; a
live failover with a worker heartbeat changing during execution remains open.

