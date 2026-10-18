# Track 1 qualification: composite runtime-filter component prefilter

## Scope

The bounded build-side runtime filter now caches the distinct values present
in each component of a composite hash key. A probe row is rejected when one
component is absent from its corresponding set; rows that pass this superset
test still use the exact composite tuple index. This reduces unnecessary
tuple construction and hash probes while preserving composite-key semantics.

## Exact local result

The executor regression uses build tuples `(1, 'x')` and `(2, 'y')` and probe
rows `(1, 'x')`, `(1, 'y')`, `(3, 'z')`, and `(NULL, 'x')`. The resulting mask
is `[true, false, false, false]`.

The `(1, 'y')` row proves that component membership is only a safe superset:
both individual values exist in the build, but the tuple does not. The NULL
row proves that SQL join-key NULL semantics remain fail-closed.

## Validation

```text
cargo fmt --package kaveon-exec -- --check
cargo test -p kaveon-exec composite_runtime_filter_prefilters_components_but_keeps_tuple_exactness -- --nocapture
```

Both checks pass on 2026-10-18. This is local executor evidence only; it does
not qualify cloud object-store pruning or a deployed multi-worker image.

