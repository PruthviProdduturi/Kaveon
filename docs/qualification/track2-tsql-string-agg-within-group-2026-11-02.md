# Bounded `STRING_AGG WITHIN GROUP` qualification — 2026-11-02

Kaveon now supports the bounded SQL Server form:

```sql
STRING_AGG(value, literal_separator)
  WITHIN GROUP (ORDER BY value [ASC|DESC])
```

The order key must be exactly the value expression. This restriction is
intentional: it lets each worker retain the values in a bounded merge state,
then sort once after all partial states arrive. Dynamic separators, `DISTINCT`,
multiple order keys, a different order expression, and explicit `NULLS FIRST`
or `NULLS LAST` remain fail-closed.

## Evidence

- SQL planner regression: `lowers_bounded_ordered_string_agg_and_rejects_unsafe_order_keys`.
- Executor regression: `ordered_string_agg_sorts_across_partial_merges_and_round_trips`.
  It verifies ascending and descending output, worker-order independence, and
  both Arrow accumulator encodings.
- Three-worker differential sweep:
  `KAVEON_DIFF_ONLY=tsql_string_agg_within_group cargo test -p kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`.
  The same sweep passed for `tsql_string_agg_within_group_desc`.

The ordered state is bounded to 1,000,000 values and a 16 MiB materialized
result per group. NULL inputs remain omitted, matching the existing unordered
`STRING_AGG` contract. The implementation is Engine-native and does not claim
general ordered-set aggregate support.

## Remaining boundary

General `WITHIN GROUP` ordering by an independent expression, multiple keys,
collation-aware ordering, and unbounded/spillable ordered aggregate state still
require separate design and qualification.
