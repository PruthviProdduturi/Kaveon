# Boolean array extrema qualification — 2026-10-18

Kaveon evaluates `ARRAY_MIN` and `ARRAY_MAX` over nullable Boolean arrays with
Trino ordering (`false < true`) and ignores null elements. A null array remains
null and an all-null array produces null.

The executor regression `array_extrema_accept_boolean_lists` covers
`ARRAY[true, NULL, false]`. The server differential corpus also runs the same
query through the three-worker fragment path and compares it with local
execution.

Command:

```text
cargo test -p kaveon-exec array_extrema_accept_boolean_lists -- --nocapture
cargo test -p kaveon-server differential_tests::distributed_differential -- --nocapture
```
