# Track 2: recursive nested MAP/LIST casts

The expression evaluator now handles recursively nested `MAP` and `ARRAY`
casts by casting child arrays and rebuilding the Arrow offsets and validity
buffers. This avoids relying on the generic Arrow cast kernel to recursively
rewrite complex children. The local regression
`expr_eval::tests::nested_map_cast_recursively_casts_list_values` casts a
`map(varchar, array(integer))` value to
`map(varchar, array(bigint))` and verifies the nested values and output type.

Validation:

```text
cargo test -p kaveon-exec nested_map_cast_recursively_casts_list_values -- --nocapture
```

This qualification is deliberately bounded. The current SQL parser does not
accept nested complex type members in a `CAST` type declaration (for example,
`MAP(VARCHAR, ARRAY(BIGINT))`), so the recursive evaluator path is currently
reachable from the internal typed expression/planner representation rather
than through that SQL spelling. It does not claim general ROW casts, arbitrary
large-list target casts, Decimal256 casts, external-client wire coverage, or
cloud/distributed qualification. Parser support for nested type declarations
is a separate follow-up boundary.
