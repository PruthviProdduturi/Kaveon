# Track 3 — physical-plan estimate lineage — 2026-10-29

## Boundary

Physical plan display now carries conservative source-estimate lineage beyond
scan nodes. Unary operators expose their known child input row count; joins
and semi/anti joins expose left and right input estimates; set operations
expose the sum of known child inputs. Each value is marked
`estimate_scope=child_input_only` (or `child_inputs_only`).

These fields are diagnostic metadata only. Kaveon does not present them as
post-filter, post-join, or post-aggregation output cardinalities, and they do
not alter execution or join selection. `TopN`, `LIMIT`, and `OFFSET` additionally
expose a conservative bounded output cap derived from the child estimate and
the operator's literal bound. Row-preserving `Filter`, `Project`, `Sort`, and
`Window` nodes propagate the same conservative child upper bound so a bounded
operator above them can remain safely capped; the filter predicate itself is
not estimated. Join nodes use the saturating product of child bounds; semi/anti
joins use the left bound; `UNION`, `INTERSECT`, and `EXCEPT` use their
mathematically safe sum/minimum/left-input bounds. Aggregate, hash-aggregate,
final-aggregate, and distinct nodes now expose the same safe child-row upper
bound; this is still a bound, not a group-cardinality estimate. None of these
values claim join selectivity.

Finished query records now annotate the physical-plan root with
`actual_output_rows` and `actual_rows_scope=query_result` when a result row
count is available. This is measured delivered-result telemetry only; it does
not pretend to attribute rows to child operators.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server plan_estimates_expose_conservative_join_input_lineage -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
```

The focused test and workspace clippy gate pass on 2026-10-30. Exact
operator-output estimates for filters, joins, and aggregates, plus selectivity
attribution, remain a separate open qualification boundary.
