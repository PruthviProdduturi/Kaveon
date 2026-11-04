# SQL Server `TIME` cast qualification — 2026-11-03

## Scope

This gate qualifies bounded `CAST(expression AS TIME)`,
`TRY_CAST(expression AS TIME)`, `CONVERT(TIME, expression)`, and
`TRY_CONVERT(TIME, expression)` for UTF-8 text and existing Arrow time values.
Results use Arrow `Time64(Microsecond)` and preserve SQL NULLs. Strict casts
reject malformed text; `TRY_CAST` converts malformed text to typed NULL.

Numeric coercions, timestamp-to-time wall-clock conversion, timezone rules, and
precision beyond the Engine's microsecond representation are outside this
qualification.

## Evidence

Focused planner regression:

```text
cargo test -p kaveon-sql lowers_time_casts_to_microsecond_time_targets -- --nocapture
```

Focused evaluator regression:

```text
cargo test -p kaveon-exec time_casts_parse_text_and_try_cast_nulls_invalid_values -- --nocapture
```

The evaluator test verifies a fractional value, strict NULL preservation,
safe NULL-on-error behavior, and strict rejection of malformed text.

Distributed exact-result regression:

```text
cargo test -p kaveon-server distributed_tsql_time_cast_matches_local -- --nocapture
```

The three-worker result is compared byte-for-byte with local execution for
literal strict casts and row-local safe casts through both SQL Server spelling
families.

## Corpus boundary

The authoritative corpus entry is `time_cast`. The implementation is pinned
by the corpus `engine_commit`; broader SQL Server temporal coercion remains a
separate qualification rather than an implied compatibility claim.
