# Track 1 — Incremental global TopN merge qualification

## Scope

The distributed TopN coordinator now folds each completed worker candidate into one bounded global candidate. It no longer retains every worker batch until all tasks finish. Because each worker contributes at most the requested limit, repeated `merge_top_n` operations preserve the exact global TopN.

## Evidence

```text
cargo test -p kaveon-exec --lib topn::tests::global_top_n_candidate_merge_is_incremental_and_exact --quiet
cargo check -p kaveon-server --all-targets
cargo fmt --all -- --check
cargo clippy -p kaveon-server --all-targets -- -D warnings
```

The regression compares a three-worker incremental merge with a single merge over all workers and requires identical ordered `RecordBatch` output. The server path keeps only the bounded candidate while worker tasks complete.

## Boundary

This reduces coordinator candidate retention but does not make the final ordered result a fully streaming network response. A live multi-worker latency and memory qualification remains required.
