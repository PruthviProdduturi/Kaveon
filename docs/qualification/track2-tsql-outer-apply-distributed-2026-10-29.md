# Bounded T-SQL `OUTER APPLY` distributed qualification — 2026-10-29

## Scope

Kaveon accepts an uncorrelated derived `OUTER APPLY` over a parenthesized
`SELECT` or `VALUES` relation, plus a correlated scalar `VALUES` relation whose
expressions bind columns from the outer row. The parser lowers both forms to
`LEFT JOIN LATERAL ... ON TRUE`; an empty derived result preserves the outer
row with NULL-valued right columns across local and three-worker execution.

## Evidence

- Parser: `bounded_outer_apply_lowers_to_left_lateral_join`
- Parser: `correlated_outer_apply_lowers_to_left_lateral_join`
- Three-worker differential: `distributed_bounded_outer_apply_preserves_empty_outer_rows`
- Three-worker differential: `tsql_correlated_outer_apply`
- The focused parser and server tests pass on the implementation commit.

## Boundary

The stage graph uses the existing broadcast/left-lateral paths and preserves
the outer row on an empty result. Arbitrary correlated derived `SELECT` forms,
`OUTER APPLY` over table-valued functions, and unbounded forms remain
fail-closed.
