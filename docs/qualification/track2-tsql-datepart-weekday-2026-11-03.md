# T-SQL `DATEPART` weekday qualification — 2026-11-03

## Scope

This qualification covers SQL Server's bounded `DATEPART(dw|weekday|w,
date_expression)` weekday spellings and the full `DATEPART(week,
date_expression)` spelling with the default `DATEFIRST 7` convention:
Sunday is `1` and Saturday is `7`. Kaveon does not expose `SET DATEFIRST` in
this contract, so the default is explicit. Generic Trino/Engine `DATE_PART`
semantics remain unchanged.

The implementation keeps the dialects distinct in the logical plan
(`DATE_PART_TSQL`) and applies the offset only to the T-SQL weekday field.
Other date parts and all supported temporal input types use the existing typed
extractor.

## Evidence

Engine commit: `4821b81b`.

Focused checks:

```text
cargo test -p kaveon-exec --lib date_part_accepts_tsql_weekday_and_dayofyear_aliases -- --quiet
cargo test -p kaveon-server --bin kaveon-server differential_tests::distributed_tsql_datepart_weekday_uses_default_datefirst_numbering -- --quiet
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --quiet
cargo test -p kaveon-sql --all-targets -- --quiet
cargo fmt --all -- --check
```

Observed result: all focused tests passed; the local evaluator and the
three-worker fragment path returned identical rows for a deterministic
`DATEPART(dw, event_date)` and `DATEPART(week, event_date)` queries. The distributed regression orders by both
the fixture key and date so tied-key TopN output cannot hide a semantic
difference.

## Boundary

`SET DATEFIRST` and session-specific weekday numbering are not implemented and
must remain an explicit unsupported shape. This is a read-compatibility slice,
not a claim of complete SQL Server date/session parity.
