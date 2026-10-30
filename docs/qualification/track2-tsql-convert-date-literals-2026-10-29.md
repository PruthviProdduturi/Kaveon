# T-SQL literal `CONVERT(DATE, …, style)` qualification — 2026-10-29

Status: **qualified for the bounded read contract**.

The Engine now has exact local/three-worker evidence for the declared literal
date styles: US numeric style `101`, named text style `0`, and compact ISO
style `112`.

## Evidence

- Focused differential: `distributed_tsql_convert_date_literals_match_local`.
- Corpus boundary: `versioned_tsql_corpus_matches_declared_boundaries`.
- The comparison uses exact canonical rows and verifies the query memory pool
  returns to zero after distributed execution.

## Boundary

This qualifies the three literal styles above. Runtime-column parsing and
other conversion styles remain governed by their separate corpus entries; no
general Fabric/Azure SQL conversion parity is implied.
