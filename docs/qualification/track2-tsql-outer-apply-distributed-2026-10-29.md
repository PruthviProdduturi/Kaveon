# Bounded T-SQL `OUTER APPLY` distributed qualification — 2026-10-29

## Scope

Kaveon accepts an uncorrelated derived `OUTER APPLY` over a parenthesized
`SELECT` or `VALUES` relation. The parser lowers it to `LEFT JOIN LATERAL ...
ON TRUE`, and an empty derived result preserves the outer row with NULL-valued
right columns across local and three-worker execution.

## Evidence

- Parser: `bounded_outer_apply_lowers_to_left_lateral_join`
- Three-worker differential: `distributed_bounded_outer_apply_preserves_empty_outer_rows`
- The focused parser and server tests pass on the implementation commit.

## Boundary

The stage graph uses a broadcast right derived relation and preserves the
outer row on an empty result. Correlated derived APPLY, `OUTER APPLY` over
table-valued functions, and unbounded forms remain fail-closed.
