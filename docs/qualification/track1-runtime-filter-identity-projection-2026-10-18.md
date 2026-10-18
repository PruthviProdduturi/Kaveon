# Track 1 runtime filtering through identity projections — 2026-10-18

## Boundary

The partitioned join runtime filter is exact before probing, but a projection
between the probe scan and join previously hid the filter from storage. That
forced the engine to decode the probe rows and apply the filter after the
projection, even when the projection was only `source_column AS alias`.

## Change

`ProjectOperator` now rewrites an incoming storage predicate through identity
column projections, including aliases. Computed expressions decline the push,
so the existing batch-level runtime filter remains the correctness fallback.
Compound predicates are rewritten recursively only when every referenced
column is an identity projection.

## Evidence

The direct rewrite regression and the existing typed runtime-filter executor
gate pass locally. The new three-worker differential compares local and
distributed results for a Parquet probe scan behind an identity projection:

```text
cargo test -p kaveon-exec project::tests::runtime_predicate_rewrites_through_identity_alias_only -- --nocapture
cargo test -p kaveon-exec partitioned::tests::partitioned_join_applies_typed_int32_runtime_filter_before_probe -- --nocapture
cargo test -p kaveon-server distributed_runtime_filter_crosses_probe_identity_projection -- --nocapture
```

All three focused tests passed. This is local/in-process distributed evidence;
remote adaptive filter exchange and cloud qualification remain open.
