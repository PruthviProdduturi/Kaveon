# Track 2 — T-SQL text conversion

**Status:** qualified for the bounded UTF-8 text contract  
**Implementation:** `3187d01d`
**Date:** 2026-11-03

## Scope

The Engine now qualifies the SQL Server/Fabric spellings
`CONVERT(VARCHAR, expression)`, `TRY_CONVERT(VARCHAR, expression)`, and
`TRY_CAST(expression AS VARCHAR)` for
scalar expressions whose result is represented as Arrow UTF-8 text. The
qualification covers numeric-to-text conversion through the existing typed
cast path, including nullable values and distributed fragment execution.
The planner also accepts unbounded `NVARCHAR`/`TRY_CONVERT(NVARCHAR, ...)` as
the same UTF-8 text target. The alias is included in the exact local/three-
worker differential corpus.

This is a logical text contract, not SQL Server code-page or `nvarchar`
storage emulation. Sized `VARCHAR(n)`, `CHAR(n)`, and equivalent text casts are
rejected explicitly: the planner must not accept a length and silently drop it.
Length limits, binary conversion styles, collation-specific
formatting, and locale-sensitive date formatting remain outside the boundary
and continue to fail closed or use their separately qualified temporal forms.

## Evidence

The authoritative corpus adds:

- `convert_varchar`
- `try_convert_varchar`
- `try_cast_varchar`
- `try_convert_nvarchar`

All four cases are included in
`engine/crates/server/src/differential_tests.rs` and were run by:

```text
cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths \
  -- --nocapture
```

Result:

```text
1 passed; 0 failed; 696 filtered out; finished in 651.76s
```

The sweep compares local execution with the three-worker fragment path across
dictionary, plain, multi-file, partitioned, hive-ordered, and clustered
Parquet layouts. The corpus is now 149 cases and is pinned to `3187d01d`.

The explicit rejection boundary is covered by the SQL planner regression
`rejects_sized_tsql_text_casts_instead_of_dropping_length`, which checks
`CAST`, `TRY_CAST`, `CONVERT`, and `TRY_CONVERT` sized text forms. This is a
fail-closed contract test rather than a distributed result qualification:
length-aware truncation and padding remain a separate implementation phase.

