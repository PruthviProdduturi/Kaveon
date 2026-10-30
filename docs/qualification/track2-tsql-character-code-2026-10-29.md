# T-SQL character-code helpers — 2026-10-29

## Scope

This qualification covers the bounded SQL Server spellings `ASCII`, `UNICODE`,
`CHAR`, and `NCHAR` on nullable UTF-8 text and integer columns. It does not claim SQL
Server locale or code-page conversion behavior.

- `ASCII(text)` returns the leading Unicode scalar when it is in the ASCII
  range (0–127). A NULL or empty value, or a leading scalar above 127, returns
  NULL rather than applying an implicit code-page conversion.
- `UNICODE(text)` returns the leading Unicode scalar value. NULL and empty
  values return NULL.
- `CHAR(integer)` accepts the bounded byte range 0–255 and returns a one-byte
  text value. NULL and out-of-range values return NULL.
- `NCHAR(integer)` accepts valid Unicode scalar values and returns the
  corresponding text value. NULL, negative, surrogate, and out-of-range values
  return NULL.

## Evidence

- Executor unit test: `evaluates_sql_server_character_code_helpers` in
  `engine/crates/exec/src/expr_eval.rs`.
- Focused three-worker differential:
  `differential_tests::distributed_tsql_character_code_helpers_match_local`.
- Full Parquet-layout and execution-path differential sweep:
  `differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`.

The focused tests compare local and three-worker results for deterministic
ordering, NULL propagation, ASCII and non-ASCII text, Unicode scalar values,
and bounded `CHAR`/`NCHAR` inputs. The full Parquet-layout and execution-path
sweep passed in 624.69 seconds.

## Release boundary

The functions are planner-qualified and distributed-qualified only within the
semantics above. Full SQL Server collation, locale, code-page, binary, and
legacy `CHAR(0)` behavior remain outside the contract and must not be inferred
from this record.
