# Distributed reliability subset — 2026-10-20

Status: **passed locally**

The current source passed the focused reliability subset below. Every command
returned zero and every selected test passed:

```powershell
cargo test -p kaveon-server --all-targets --quiet a_worker_lost
cargo test -p kaveon-server --all-targets --quiet a_worker_loss
cargo test -p kaveon-server --all-targets --quiet a_distributed_
cargo test -p kaveon-server --all-targets --quiet a_queued_statement_can_be_cancelled_by_id
cargo test -p kaveon-server --all-targets --quiet a_restart_puts_the_query_history_back_in_the_store
```

The aggregate was **9 passed, 0 failed**:

- three worker-loss recovery tests covering producer/consumer loss, producer
  loss, and distributed join recovery;
- one grouped-aggregate global-order worker-loss test;
- three distributed spill/refusal/cleanup tests;
- queued statement cancellation; and
- query-history restoration after coordinator restart.

This is a source-level local reliability gate. It does not qualify AKS node
drains, remote object-store faults, or sustained cloud endurance.

