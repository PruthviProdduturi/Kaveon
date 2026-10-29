# T-SQL `CONCAT` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

`CONCAT` uses SQL Server NULL-as-empty semantics through the existing
vectorized string evaluator. This qualification covers scalar argument lists
over the declared corpus shape, including nullable text columns.

## Evidence

- Focused distributed differential:
  `distributed_tsql_concat_matches_local_with_null_fields`.
- Corpus boundary:
  `versioned_tsql_corpus_matches_declared_boundaries`.
- The three-worker result is compared exactly with the local result and the
  query memory pool returns to zero after execution.

## Boundary

This qualifies scalar `CONCAT` over the declared arguments and does not claim
general procedural T-SQL or unrestricted Fabric Warehouse parity.
