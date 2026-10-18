# Distributed Boolean aggregate extrema qualification

## Scope

The coordinator's distributed partial-aggregate merge now compares Boolean
`MIN` and `MAX` values using the same ordering as the local evaluator:
`false < true`. Previously the merge accepted numeric and text extrema but
rejected Boolean values as incomparable.

## Evidence

The server regression `merges_boolean_min_max_partials` merges two worker
partials for the same group and verifies the exact result `MIN = false` and
`MAX = true`.

## Boundary

This covers Boolean partial merge ordering only. It does not alter aggregate
spill admission, high-cardinality merge behavior, or cloud qualification.
