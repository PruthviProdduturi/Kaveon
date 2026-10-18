# Track 1 remote exchange upload retry — 2026-10-18

## Boundary

Remote Arrow exchange uploads now retry a bounded set of transient worker
failures. Each chunk is encoded once and may be sent up to three times with
25 ms and 50 ms backoff. Transport failures and HTTP 5xx responses are
retryable; 4xx responses remain terminal so authorization, cancellation, and
protocol errors fail immediately. The retry is per chunk, so a completed
chunk is not resent when a different chunk fails.

## Evidence

The server unit suite verifies the retry classification and runs an in-process
Axum worker that returns two `503 Service Unavailable` responses followed by
`202 Accepted`; `upload_chunks` completes successfully after exactly three
requests. The focused command is:

```text
cargo test -p kaveon-server exchange::tests::upload_retries -- --nocapture
```

Result: 2 passed, 0 failed.

## Remaining boundary

This is bounded client-side retry protection for transient exchange upload
failures. It does not claim cross-worker replication, durable remote spill, or
multi-coordinator failover. Those remain deployment and recovery gates.
