# Track 2 — T-SQL `QUOTENAME`

## Scope

`QUOTENAME(value [, quote_character])` safely quotes an identifier-shaped
string. Kaveon's qualified surface uses SQL Server's default square brackets
and the double-quote delimiter, preserves NULLs, doubles a closing delimiter,
and returns NULL for values longer than 128 Unicode characters.

## Evidence

The local evaluator covers default brackets, a doubled closing bracket,
double-quote mode, and NULL propagation:

```text
cargo test -p kaveon-exec quotename -- --nocapture
```

The distributed differential corpus covers the default bracket form across
dictionary, plain, partitioned, clustered, and three-worker Parquet paths:

```text
KAVEON_DIFF_ONLY=tsql_quotename \
  cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
```

Both gates pass on the 2026-10-29 Engine tree.

## Boundary

Only bracket and double-quote delimiters are qualified. Other SQL Server quote
characters and dynamic identifier semantics remain explicitly unsupported.
