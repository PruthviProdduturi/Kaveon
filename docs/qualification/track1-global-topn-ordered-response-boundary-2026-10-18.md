# Track 1 — global TopN ordered-response boundary — 2026-10-18

## Scope

The distributed `ORDER BY … LIMIT` path is bounded during execution: each
worker returns at most the requested limit, and the coordinator merges those
candidates without reading the full input. The remaining Track 1 item is a
fully parallel ordered result stream. This note records the current protocol
boundary so that work is not reported as complete by inference from the
bounded merge.

## Current contract

The distributed TopN dispatcher in `engine/crates/server/src/api.rs` submits
worker tasks with `stream_result: false`. Each worker returns a
`TaskResponse`, whose `data` field is `Vec<Vec<serde_json::Value>>`. The
coordinator must therefore receive a complete bounded candidate from every
partition before it can perform the global merge. It then converts the merged
Arrow batch to the same JSON row representation and returns it in
`StatementResponse.data`.

Paged delivery changes where the completed rows are stored, not when they
become available: `spool_rows` drains the already-materialized JSON rows into
the result writer after `execute_distributed_top_n` returns. The existing
result-page endpoint can stream page retrieval to a client, but it cannot
publish the first globally ordered page while an unobserved worker could still
produce a smaller key.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server api::tests::distributed_top_n_parallel_tree_keeps_global_order_and_bound -- --nocapture
cargo test -p kaveon-server --bin kaveon-server api::tests::distributed_top_n_keeps_the_schema_when_all_workers_are_empty -- --nocapture
cargo fmt --all -- --check
cargo clippy -p kaveon-server --all-targets -- -D warnings
```

The two server regressions prove the bounded global candidate and empty-result
schema contract. The implementation is exact for ordering and ties, but the
HTTP response remains a materialized JSON result (or a completed paged result
spool).

## Boundary and next implementation

Do not claim a fully parallel ordered result stream until the worker/coordinator
protocol carries ordered Arrow batches with an end-of-stream marker and the
coordinator has a cancellation-safe k-way merge that can publish pages only
after the necessary lower-bound frontier is known. That protocol change must
also define failure-after-first-page, retry, `WITH TIES`, and result-cache
semantics. Changing only the merge tree or result-page endpoint would either
break global ordering or change the existing response contract.
