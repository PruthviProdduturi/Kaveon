# Track 2 `WIDTH_BUCKET` qualification

## Scope

This qualification covers the bounded scalar `WIDTH_BUCKET(value, min, max,
bucket_count)` implementation. It follows Trino's bucket boundaries: values
below `min` return zero, values at or above `max` return `bucket_count + 1`,
interior values return a one-based bucket, and NULL inputs return NULL. The
engine rejects a non-positive bucket count or an invalid range where `min >=
max`.

## Evidence

- Commit: `cdc4fc3f`
- Focused test: `cargo test -p kaveon-exec width_bucket_matches_trino_boundaries_and_nulls --quiet`
- Result: 1 passed, 0 failed
- Static gates: `cargo clippy --workspace --all-targets -- -D warnings` passed;
  `cargo fmt --all -- --check` passed after formatting; documentation validation
  passed.

This is a local scalar-function qualification. It does not claim a distributed
wire or broad external-client qualification; those remain part of the broader
Track 2 differential corpus.

The same execution slice also qualifies the numeric `IS_FINITE`, `IS_INFINITE`,
and `IS_NAN` predicates, including positive/negative infinity, NaN, and NULL
inputs. Their regression is
`cargo test -p kaveon-exec numeric_finiteness_predicates_preserve_nulls --quiet`.
