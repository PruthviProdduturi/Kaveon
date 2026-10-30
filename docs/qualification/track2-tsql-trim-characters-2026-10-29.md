# T-SQL `TRIM` character-set qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

`TRIM(BOTH characters FROM value)` now lowers to the vectorized text path and
removes any leading or trailing Unicode scalar contained in the character
set. NULL input or NULL character sets remain NULL. The one-argument form is
unchanged; `LEADING`, `TRAILING`, and comma-separated character-list forms
remain explicitly unsupported.

## Evidence

- Focused differential: `distributed_tsql_trim_characters_matches_local`.
- Corpus boundary: `versioned_tsql_corpus_matches_declared_boundaries`.
- The exact local/three-worker comparison returns the memory pool to zero.

## Boundary

This qualifies BOTH-mode character-set trimming only; it is not a claim of
general SQL Server string-function parity.
