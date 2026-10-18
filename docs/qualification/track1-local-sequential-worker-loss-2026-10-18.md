# Track 1 local sequential worker-loss gate — 2026-10-18

This is a bounded local recovery gate. It verifies that one query can survive
two sequential worker losses while the coordinator keeps the remaining worker
assignments and retry budget consistent. It does not claim node-drain,
network-fault, or long-running cluster endurance.

## Evidence

| Gate | Command | Result |
| --- | --- | --- |
| Sequential worker-loss orchestration | `cargo test -p kaveon-server --bin kaveon-server sequential_worker_losses_recover_the_same_query_on_the_last_survivor --quiet` | 1 passed |

The test runs a two-stage hash exchange over three workers. It starts both
consumer partitions, removes `worker-b`, rebuilds the producer stage, then
removes `worker-c` and rebuilds it again on the final `worker-a` survivor. It
asserts both loss records, the replayed stage and task attempts, the one-worker
survivor set, and terminal completion after the retained consumer finishes.

The per-query stage retry limit is set to two; the test therefore also proves
that the second loss remains within the configured budget rather than silently
retrying without a bound.

## Scope boundary

This is control-plane and exact-state evidence in one process. AKS node-drain,
repeated-loss endurance under pressure, network failure, and object-store
exchange recovery remain open qualification gates.
