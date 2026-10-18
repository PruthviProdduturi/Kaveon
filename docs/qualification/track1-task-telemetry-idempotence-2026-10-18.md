# Track 1 task telemetry idempotence qualification

## Scope

Live distributed query records retain one telemetry row per stage partition.
If a partition completion is published more than once, the later completion
replaces the earlier row instead of inflating task counts and scan totals.
This keeps retry/update reporting bounded and exact without changing task
dispatch or scheduler behavior.

## Evidence

- `cargo fmt --all -- --check`: passed
- `cargo test -p kaveon-server --bin kaveon-server retrying_a_partition -- --nocapture`: 1 passed, 0 failed
- `cargo test -p kaveon-server --bin kaveon-server a_running_record -- --nocapture`: 1 passed, 0 failed
- `git diff --check`: passed

This is local control-plane telemetry evidence. It does not claim a new AKS
topology, workload, or distributed execution SLO qualification.
