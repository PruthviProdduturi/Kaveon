# SQL-standard/Trino LISTAGG qualification — 2026-11-03

## Scope

Kaveon accepts the bounded LISTAGG surface and lowers it to the existing
mergeable STRING_AGG state:

    SELECT LISTAGG(name, ',')
           WITHIN GROUP (ORDER BY name)
    FROM t;

    SELECT LISTAGG(name)
           WITHIN GROUP (ORDER BY name)
    FROM t;

The separator is a text literal no larger than 64 KiB; when omitted it is the
empty string. The only supported WITHIN GROUP key is the value expression,
with ascending or descending order. NULL values retain STRING_AGG semantics
and are omitted.

ON OVERFLOW clauses, dynamic separators, multiple order keys, explicit NULL
placement, and unbounded ordered state remain rejected explicitly. This is a
bounded compatibility alias, not a claim of complete SQL-standard LISTAGG
overflow behavior.

## Evidence

- cargo test -p kaveon-sql --lib lowers_bounded_string_agg_to_mergeable_aggregate
  — parser/planner lowering, empty default separator, and overflow rejection.
- cargo test -p kaveon-cli --bin kaveon executes_bounded_string_agg_with_literal_separator
  — local execution with explicit and default separators.
- KAVEON_DIFF_ONLY=tsql_listagg cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
  — exact local versus three-worker results across dictionary, plain,
  multi-file, partitioned, and clustered Parquet layouts.

The implementation reuses the already-qualified STRING_AGG distributed state;
no new merge or spill semantics are introduced.
