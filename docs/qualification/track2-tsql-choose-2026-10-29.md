# T-SQL `CHOOSE` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

`CHOOSE(index, value1, ...)` is lowered to the existing vectorized CASE path.
Indexes are one-based; NULL and out-of-range indexes produce NULL. The
lowering is bounded by the supplied argument list and does not introduce a
new runtime operator.

## Evidence

- Implementation: `a1d684fb`.
- Planner regression: `lowers_tsql_choose_to_null_preserving_case`.
- Distributed differential: `distributed_tsql_choose_matches_local`, with
  exact local-vs-three-worker rows and zero retained memory reservations.
- Corpus boundary: `versioned_tsql_corpus_matches_declared_boundaries`.
- Strict workspace Clippy and documentation validation passed.

## Boundary

This qualifies scalar `CHOOSE` expressions over the declared argument list;
general procedural T-SQL and dynamic SQL remain outside the contract.
