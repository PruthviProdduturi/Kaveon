# T-SQL `STRING_AGG(DISTINCT ...)` qualification — 2026-11-03

## Status

Qualified for the bounded Engine read surface. This is not a claim of full
SQL Server ordered-set aggregate compatibility.

## Contract

The Engine accepts a text value column, a literal text separator, and the
`DISTINCT` modifier:

```sql
SELECT STRING_AGG(DISTINCT name, ',') FROM t;
```

The planner lowers the query to a projection of the grouping keys plus `name`,
an exact `DISTINCT` stage, and the existing mergeable `STRING_AGG` state. The
value is therefore deduplicated globally before partial aggregation; duplicate
values split across workers cannot reappear at the final merge. With no
`GROUP BY`, the projection contains only the value column. With ordinary column
grouping, the grouping keys remain in the distinct key so each group is
deduplicated independently.

NULL values retain the existing `STRING_AGG` behavior and are omitted. The
existing per-group output and state bounds remain in force. Ordered
`WITHIN GROUP` forms use the same rewrite when the order key is the aggregate
value column.

## Rejections and boundaries

The separator must remain a text literal no longer than 64 KiB. Arbitrary
expressions, dynamic separators, unsupported order keys, and non-column
aggregate arguments remain rejected by the existing bounded contract. The
rewrite is intentionally limited to a single distinct `STRING_AGG` aggregate;
mixed aggregate lists and filtered expression forms remain outside this gate
until they receive their own exact lowering and distributed evidence.

## Evidence

- `logical_plan::tests::lowers_bounded_string_agg_to_mergeable_aggregate`
  — parser/planner rewrite and distinct projection.
- `local::tests::executes_bounded_string_agg_with_literal_separator` — local
  duplicate-value result (`click|view` versus the non-distinct
  `click|click|view`).
- `cargo test -p kaveon-sql --lib lowers_bounded_string_agg_to_mergeable_aggregate`
  — 1 passed, 0 failed on 2026-11-03.
- `cargo test -p kaveon-cli --bin kaveon executes_bounded_string_agg_with_literal_separator`
  — 1 passed, 0 failed on 2026-11-03.
- `docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json` case
  `string_agg_distinct`, pinned to the implementation commit.

Distributed qualification is still required before this is promoted beyond
the bounded read gate; the rewrite is designed to use the existing exact
distinct exchange path, but no multi-worker result is claimed by this document.
