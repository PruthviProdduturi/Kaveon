# Track 2 null-safe complex comparisons

**Date:** 2026-10-16  
**Source:** `origin/dev` at `5ce87c76`

Kaveon now evaluates `IS DISTINCT FROM` and `IS NOT DISTINCT FROM` through a
recursive null-safe comparison path. This preserves SQL's two-valued semantics
inside arrays, maps, and rows while ordinary `=` and `<>` retain three-valued
semantics.

The focused execution regression covers:

- arrays containing NULL elements;
- ROW values;
- MAP values; and
- the inverse `IS DISTINCT FROM` result.

The focused gates passed:

```text
cargo test -p kaveon-exec null_safe_comparison_recurses_through_arrays_rows_and_maps -- --nocapture
1 passed, 0 failed

cargo test -p kaveon-sql --lib --no-fail-fast --quiet
185 passed, 0 failed

cargo test -p kaveon-exec --lib --no-fail-fast --quiet
381 passed, 0 failed, 4 ignored

cargo clippy --locked -p kaveon-exec -p kaveon-sql --all-targets -- -D warnings
passed

cargo fmt --all -- --check
passed

node scripts/validate-docs.mjs
193 Markdown files, 39 routes, 9 SVGs; passed
```

This evidence qualifies nested null-safe comparison behavior in the execution
and SQL unit suites. It does not expand the remaining Track 2 gaps around full
complex-type coercion, ordered MAP semantics, or external-client qualification.
