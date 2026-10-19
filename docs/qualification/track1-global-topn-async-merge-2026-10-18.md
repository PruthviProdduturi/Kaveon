# Track 1 — Async global TopN coordinator merge qualification

## Scope

The distributed TopN coordinator now runs each bounded Arrow candidate merge on Tokio's blocking pool. Worker requests and control-plane tasks therefore do not share an async runtime thread with CPU-heavy key comparison and payload selection. The merge remains associative, retains at most the requested limit, and produces the same ordered result.

## Evidence

```text
cargo test -p kaveon-exec --lib topn::tests::global_top_n_candidate_merge_is_incremental_and_exact --quiet
cargo test -p kaveon-server --bin kaveon-server api::tests::distributed_top_n_keeps_the_schema_when_all_workers_are_empty --quiet
cargo check -p kaveon-server --all-targets
cargo fmt --all -- --check
cargo clippy -p kaveon-server --all-targets -- -D warnings
```

The existing three-worker incremental merge regression remains the correctness oracle; the server empty-worker regression verifies schema preservation through the async dispatch path. The change is scheduling-only: it moves candidate ranking off the async executor without changing the candidate bound or comparator contract.

## Boundary

The ordered candidate reduction is still serialized to preserve the bounded-memory invariant, and the final ordered result is not yet a fully parallel streaming response. A live latency and concurrency qualification on a deployed multi-worker topology remains required.