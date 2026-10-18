# Track 1 — single-key runtime-filter predicate qualification

## Scope

Single-key build-side runtime filters now emit each value once when converted
to a storage `IN` predicate. The hash index already keeps every distinct key
in `first_rows`; duplicate build rows are retained separately only for join
output semantics. Re-emitting those duplicate keys widened storage predicates
without changing their result set, so the pushdown path now uses the distinct
key map directly for the supported typed indexes.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec runtime_filter -- --nocapture
running 7 tests
test result: ok. 7 passed; 0 failed
```

The focused regression builds an Int64 build side containing duplicate values
and NULL, converts it to a storage predicate, and verifies that the predicate
contains exactly the two distinct non-null values. The existing composite-key,
decimal256, empty-build, partitioned-inner, and partitioned-semi/anti runtime
filter tests also pass in the same run.

## Boundary

This reduces predicate size for supported single-key typed indexes; it does not
expand runtime-filter type coverage or qualify measurable cloud row-group
pruning. Composite predicates already deduplicate each independent `IN` list.
