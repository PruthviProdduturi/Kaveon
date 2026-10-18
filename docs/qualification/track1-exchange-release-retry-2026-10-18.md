# Track 1 qualification: exchange cleanup retry policy

## Scope

Coordinator cleanup now retries exchange deletion when a worker is temporarily
unreachable or returns a 5xx response. It uses three bounded attempts with
short exponential backoff. Missing exchanges, authentication failures, and
other client errors are terminal and are not retried. This prevents a worker
loss or transient network failure from silently leaving exchange files until
TTL cleanup.

## Validation

The policy regression verifies that transport failures and HTTP 5xx responses
are retryable, while 404 and unauthorized responses are terminal:

```text
cargo test -p kaveon-server exchange_cleanup_retries_only_transient_release_failures -- --nocapture
cargo test -p kaveon-server exchange_cleanup_is_enqueued_without_waiting_for_network_completion -- --nocapture
```

Both checks pass on 2026-10-18. This validates retry classification and
bounded scheduling locally; an injected live worker-loss test remains needed
to measure cleanup completion against a deployed topology.

