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

The worker-finalization path now closes the in-memory exchange store as well as
the task lifecycle. It removes every chunk for the query while holding the
store write lock and retains a bounded 15-minute terminal marker, so uploads
that were already in flight when cancellation arrived are rejected instead of
recreating orphaned exchange state. The coordinator cleanup path uses the same
finalization operation. Regression coverage is provided by
`finishing_query_releases_all_exchanges_and_rejects_late_uploads` and
`finishing_a_query_cancels_the_tasks_it_still_runs`.

This is a local runtime regression. Long-running federated cancellation under
resource pressure remains open in the parity tracker.
