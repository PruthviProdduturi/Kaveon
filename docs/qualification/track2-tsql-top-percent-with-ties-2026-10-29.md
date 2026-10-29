# T-SQL `TOP PERCENT WITH TIES` qualification — 2026-10-29

## Contract

The Engine accepts the SQL Server form `TOP n PERCENT WITH TIES` when the
statement has an `ORDER BY`. It lowers to the existing exact percent-window
contract: a total-row window, a `RANK()` over the declared order keys, and a
`CEIL(total * n / 100)` boundary. Every row at the boundary is retained.

The form without `ORDER BY` is rejected because the tied boundary is not
deterministic. Dynamic percentages, additional TOP modifiers, and unrelated
write forms remain outside this read contract.

## Evidence

- SQL planner suite: `298/298` tests passed, including the versioned corpus
  boundary test.
- Focused distributed differential (`KAVEON_DIFF_ONLY=tsql_top_percent_with_ties`):
  passed across the existing Parquet layouts and local/partitioned execution
  paths in `43.03s`, with exact local-versus-distributed results.
- Clippy passed for `kaveon-sql` and `kaveon-server` with `-D warnings`.

This qualifies the SQL spelling and the declared deterministic-order boundary;
it is not a claim of general SQL Server TOP or full T-SQL parity.
