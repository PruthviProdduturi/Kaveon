# Product read-path snapshot sharing qualification — 2026-11-03

Kaveon product reads now reuse the verified immutable catalog snapshot instead
of cloning every product and typed-row map for each request. The API uses
`ProductCatalogCommit::read_current_shared`, while the commit layer still
reads the mutable head on every request so a newly published generation is
never hidden by the cache. A snapshot is reused only when its exact immutable
reference matches the head's digest-checked reference.

Product-list document fetches are also issued with bounded concurrency (16
in-flight object reads). `buffered` preserves the catalog page order, and
authorization filtering still occurs before pagination is exposed. The result
does not change the ownership, generation, snapshot-id, digest, or corruption
contracts; it only removes avoidable snapshot copies and serialized document
round trips.

## Evidence

- `catalog::tests::shared_snapshot_reads_reuse_the_verified_snapshot` proves
  repeated shared reads return the same verified `Arc` for an unchanged head.
- `catalog::tests::a_moved_head_is_never_answered_from_the_cache` proves a
  newly published generation is observed rather than served from the prior
  cache entry.
- `transaction_api::tests::product_list_is_bounded_paginated_and_owner_isolated`
  proves pagination and owner filtering remain unchanged through the bounded
  concurrent fetch path.
- `transaction_api::tests::product_point_read_*` proves point-read ownership,
  missing-record, and corruption behavior remains unchanged.

Validation:

```text
cargo check -p kaveon-server --all-targets
cargo clippy -p kaveon-server --all-targets --no-default-features -- -D warnings
cargo test -p kaveon-catalog --lib product_commit::tests::shared_snapshot_reads_reuse_the_verified_snapshot -- --nocapture
cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::product_list_is_bounded_paginated_and_owner_isolated -- --nocapture
cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::product_point_read -- --nocapture
```

This is a local implementation qualification. It does not claim a particular
cloud latency, object-store throughput, or external-client SLO; those require
the deployment and workload gates.
