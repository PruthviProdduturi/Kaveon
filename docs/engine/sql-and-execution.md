# SQL and execution

## Committed baseline

The committed Engine executes scans, filters, projections, arithmetic and comparisons, aliases, grouped/global aggregates, exact `COUNT(DISTINCT ...)`, ordering, limits, and inner/left/right/full/cross joins. Eligible shapes execute locally and through distributed fragments.

## Active SQL milestone

The current integration adds CASE, LIKE/ILIKE, BETWEEN, IN lists, CAST, string concatenation, OFFSET, row DISTINCT, UNION ALL, HAVING, CTEs, derived tables, scalar functions, windows, set operations, and more distinct aggregates. These are not committed capability claims until Claude's combined parser/operator/fragment changes land on `dev` and the complete suite passes there.

## Definition of complete

A feature requires parser semantics, logical planning, safe optimization, null/type/error-correct physical execution, versioned fragment support where applicable, local/distributed equivalence, tests, and matching documentation. Unsupported syntax must fail explicitly. Kaveon does not claim an ANSI SQL percentage without a published conformance corpus.

Remaining broad gaps include scalar/correlated subqueries, comprehensive date/time and decimal behavior, and full standard conformance.

