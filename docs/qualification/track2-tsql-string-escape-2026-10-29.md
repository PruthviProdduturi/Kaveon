# Track 2 — T-SQL `STRING_ESCAPE` JSON mode

## Scope

`STRING_ESCAPE(value, 'json')` is a common SQL Server/Fabric read expression
for safely embedding text in JSON. Kaveon supports the JSON mode on UTF-8 text,
preserves NULL inputs, and rejects unsupported escape modes instead of silently
changing the result.

## Evidence

The local evaluator regression covers quotes, control characters, a case-
insensitive `JSON` mode literal, and NULL propagation:

```text
cargo test -p kaveon-exec evaluates_string_escape_json_and_nulls -- --nocapture
```

The distributed differential corpus also exercises the expression through
dictionary, plain, partitioned, clustered, and three-worker Parquet paths:

```text
KAVEON_DIFF_ONLY=tsql_string_escape_json \
  cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
```

Both gates pass on the 2026-10-29 Engine tree.

## Boundary

Only the SQL Server `json` escape mode is qualified. XML escaping and arbitrary
runtime escape-mode expressions remain explicitly unsupported.
