# T-SQL `PARSENAME` qualification — 2026-11-03

Kaveon now accepts the bounded SQL Server `PARSENAME(value, part)` scalar
form for plain dotted names. Part `1` selects the rightmost segment and part
`4` the leftmost segment. The evaluator returns NULL for a NULL input, NULL or
out-of-range part, empty segments, names with more than four segments, or a
segment longer than SQL Server's `sysname` limit of 128 Unicode characters.

This boundary deliberately excludes bracketed/quoted identifier parsing and
collation-specific object-name rules. Those shapes remain explicit future
compatibility work rather than being silently approximated.

Evidence:

- `expr_eval::tests::evaluates_parsename_parts_and_rejects_invalid_shapes`
  verifies right-to-left selection, malformed dotted names, and NULL input.
- `logical_plan::tests::lowers_tsql_parsename_as_a_bounded_scalar_function`
  verifies the SQL planner preserves the function for vectorized execution.
- Case `parsename` is included in the versioned T-SQL corpus as a
  `planner_qualified` case. Distributed and external-client qualification are
  intentionally still open.
