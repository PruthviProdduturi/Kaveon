# Track 1 qualification: stale worker-loss attempts

## Scope

The coordinator recovery state machine rejects a completion from a worker
attempt that was superseded by worker loss. The replacement attempt is then
allowed to replay the affected producer stage and consume only the current
attempt's exchange locations. This prevents late network responses from a
lost worker from marking a recovered task finished or publishing stale
exchange cleanup.

## Exact local result

The deterministic three-worker orchestrator gate starts a consumer, removes
its worker, submits the old attempt's completion, and asserts that completion
is rejected. It repeats that sequence after a second worker loss, proving that
both superseded attempts remain fenced. It then replays the producer stage and
completes the replacement consumer on the surviving worker. The assertions
also verify the final survivor and retry budget.

```text
cargo test -p kaveon-server sequential_worker_losses_recover_the_same_query_on_the_last_survivor -- --nocapture
```

The regression passes on 2026-10-18. This is an in-process recovery gate; it
does not qualify a live AKS node drain or object-store outage.

