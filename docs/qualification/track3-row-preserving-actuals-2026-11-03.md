# Row-preserving operator actuals — 2026-11-03

## Scope

Query records now retain a bounded estimate-vs-actual chain through physical
`Window`, `Project`, `Sort`, and the first directly reached `Filter` operator.
When the finished query has an exact result row count, that count is attached
to each directly connected row-preserving child and to that filter with
`actual_rows_scope=derived_row_preserving`.

The propagation is intentionally fail-closed. It stops at the first `Filter`
and never labels the filter's input scan with post-filter rows. It also stops
before a child of `Limit`, `Offset`, `Aggregate`, `Distinct`, a join, or a set
operation; those operators can change cardinality and still require counters
emitted by their runtime operators. No selectivity, group cardinality, or join
result is inferred from the final result.

## Evidence

The server unit `actual_rows_follow_only_row_preserving_operator_chains`
verifies that a `Window → Project → Sort → Filter → Scan` plan receives the
exact result count on the row-preserving chain and on the filter output, while
the filter's input scan does not receive a fabricated count.

```text
cargo test -p kaveon-server --bin kaveon-server \
  actual_rows_follow_only_row_preserving_operator_chains -- --nocapture
test result: ok. 1 passed; 0 failed
```

## Boundary

This is diagnostic attribution only; it does not alter planning or execution.
Full intermediate operator counters, selectivity attribution, join/aggregate
cardinality, and distributed stage resource attribution remain open until the
runtime emits and qualifies those counters.
