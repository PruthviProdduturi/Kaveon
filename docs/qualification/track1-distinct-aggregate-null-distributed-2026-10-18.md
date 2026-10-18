# Distinct aggregate NULL qualification

The distributed aggregate path preserves SQL NULL rules for distinct
aggregates. With an input containing only NULL values,
`COUNT(DISTINCT value)` returns zero while `SUM(DISTINCT value)` and
`AVG(DISTINCT value)` return NULL.

The server differential test
`distributed_distinct_aggregates_ignore_null_inputs` compares exact canonical
rows from the local operator and the in-process three-worker fragment path.
Both return `0 / NULL / NULL`, and the shared query memory pool returns to zero
after execution.

This is a correctness qualification for all-NULL distinct state merging. It
does not qualify approximate distinct sketches, mixed numeric types, or large
cardinality spill pressure.
