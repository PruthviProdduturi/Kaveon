# T-SQL `PATINDEX` qualification — 2026-10-28

## Contract

Kaveon accepts the bounded SQL Server `PATINDEX(pattern, expression)` form
for nullable UTF-8 text. `%` and `_` wildcards are supported, leading `%`
wildcards are removed before calculating the one-based Unicode scalar
position, and no match returns `0`. NULL inputs return NULL. SQL Server
bracket classes are rejected explicitly until their escaping and collation
semantics are qualified.

## Evidence

- Local executor regression:
  `patindex_uses_unicode_positions_and_bounded_wildcards`.
- Distributed differential case: `tsql_patindex` in
  `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`.
- The differential passed across the supported Parquet encodings and the
  three-worker fragment execution path.

## Boundary

This is not a claim of complete SQL Server pattern/collation compatibility.
Bracket expressions, escape clauses, and locale-specific matching remain
explicitly unsupported.

