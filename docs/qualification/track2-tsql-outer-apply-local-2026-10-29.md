# Bounded T-SQL `OUTER APPLY` local qualification — 2026-10-29

## Scope

Kaveon accepts an uncorrelated derived `OUTER APPLY` over a parenthesized
`SELECT` or `VALUES` relation. The parser lowers it to `LEFT JOIN LATERAL ...
ON TRUE`, and an empty derived result preserves the outer row with NULL-valued
right columns.

## Evidence

- Parser: `bounded_outer_apply_lowers_to_left_lateral_join`
- Local differential: `bounded_outer_apply_preserves_empty_outer_rows_locally`
- The focused parser and server tests pass on the implementation commit.

## Boundary

This is a local qualification only. Distributed OUTER APPLY is intentionally
not claimed until the stage graph can express the left-lateral derived edge
without an unconnected exchange. Correlated derived APPLY, `OUTER APPLY` over
table-valued functions, and unbounded forms remain fail-closed.
