# Track 2 — T-SQL text conversion

**Status:** qualified for the bounded UTF-8 text contract  
**Implementation:** `a8660664`
**Date:** 2026-11-03

## Scope

The Engine now qualifies the SQL Server/Fabric spellings
`CONVERT(VARCHAR, expression)`, `TRY_CONVERT(VARCHAR, expression)`, and
`TRY_CAST(expression AS VARCHAR)` for
scalar expressions whose result is represented as Arrow UTF-8 text. The
qualification covers numeric-to-text conversion through the existing typed
cast path, including nullable values and distributed fragment execution.

This is a logical text contract, not SQL Server code-page or `nvarchar`
storage emulation. Length limits, binary conversion styles, collation-specific
formatting, and locale-sensitive date formatting remain outside the boundary
and continue to fail closed or use their separately qualified temporal forms.

## Evidence

The authoritative corpus adds:

- `convert_varchar`
- `try_convert_varchar`
- `try_cast_varchar`

All three cases are included in
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
Parquet layouts. The corpus is now 148 cases and is pinned to `a8660664`.

