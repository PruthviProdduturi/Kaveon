# Track 1 multi-stage cancellation cleanup qualification

## Scope

The runtime cancellation path is checked on a three-stage graph with two Arrow
exchange edges. Cancelling a running source stage must terminate the graph and
enqueue cleanup for every exchange exactly once. A repeated cancellation must
not enqueue duplicate cleanup work.

## Evidence

- Commit: `120a9ff3`
- Test: `cancel_running_multi_stage_graph_queues_each_exchange_once`
- Command: `cargo test -p kaveon-server cancel_running_multi_stage_graph_queues_each_exchange_once --quiet`
- Result: 1 passed, 0 failed

This is a local runtime regression. Long-running federated cancellation under
resource pressure remains open in the parity tracker.
