# Global TopN Decimal128 qualification

## Scope

This qualification covers the bounded single-key `Decimal128` fast path used
by global `ORDER BY ... LIMIT` selection after fragment results are combined.
The path ranks row references against the decimal's exact unscaled `i128`
values, so the coordinator does not concatenate every payload column before
selecting the requested rows. It preserves Arrow's `NULLS FIRST`/`NULLS LAST`
placement independently of ascending or descending direction.

This is an optimization of the existing exact ordering semantics. It does not
claim Decimal256 support, scale conversion between columns, or a fully
coordinator-independent ordered result stream.

## Evidence

The focused regression is:

```text
cargo test -p kaveon-exec topn::tests::typed_decimal_topn_matches_arrow_with_nulls_and_ordering --quiet
```

The test compares ascending and descending order, both null placements, ties,
empty output, a limit larger than the input, negative values, and decimal
values with scale 2 against Arrow `lexsort_to_indices`. The current run passed
(`1 passed, 0 failed`). The executor crate also passed strict library Clippy:

```text
cargo clippy -p kaveon-exec --lib -- -D warnings
```

## Remaining boundary

Multi-key decimal ordering, Decimal256, scale conversion, cloud/deployed
qualification, and sustained distributed ordering remain covered by the
broader Track 1 gates rather than this local optimization.
