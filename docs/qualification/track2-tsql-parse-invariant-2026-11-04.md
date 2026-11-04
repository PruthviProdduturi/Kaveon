# T-SQL `PARSE` invariant-target qualification — 2026-11-04

## Boundary

Kaveon now accepts the bounded SQL Server `PARSE(value AS type)` read form
for invariant numeric and temporal targets. It lowers to the Engine's strict
typed-cast path, so malformed non-NULL values fail the statement rather than
being silently converted. `USING '<culture>'` forms remain explicitly
rejected because locale behavior is not a deterministic distributed contract.

This is a read-compatibility feature; it does not claim SQL Server CLR or
locale-aware parsing parity.

## Evidence

Planner coverage:

```text
cargo test -p kaveon-sql --lib lowers_tsql_parse_to_strict_invariant_casts
test result: ok. 1 passed; 0 failed
```

The test covers strict `BIGINT`, `DATE`, and `DATETIME2` targets and verifies
that a culture-aware `USING 'en-US'` form fails closed.

Local versus three-worker exact-result coverage:

```text
cargo test -p kaveon-server --bin kaveon-server \
  distributed_tsql_parse_matches_local -- --nocapture
test result: ok. 1 passed; 0 failed; 712 filtered out
```

The differential uses a row-local numeric value, orders by a complete key,
and compares canonical local and three-worker output with zero retained query
memory after execution.

The temporal boundary is also covered by:

```text
cargo test -p kaveon-server --bin kaveon-server \
  distributed_tsql_parse_invariant_temporal_matches_local -- --nocapture
test result: ok. 1 passed; 0 failed
```

That case compares strict `DATE` and `DATETIME2` parses of a source-backed
date column across the same three-worker path.

The authoritative corpus cases are `parse_invariant_numeric` and
`try_parse_decimal`; the compatibility tracker is synchronized at 184 cases.

The decimal safe-parse boundary also has a focused local-versus-three-worker
differential. Valid numeric text preserves `DECIMAL(10,2)` precision and scale;
malformed text, NULL, and overflow return typed NULL through the `TRY_PARSE`
path, matching the safe-cast contract.

## Explicit boundary

- `PARSE` targets must be numeric or one of the invariant temporal targets
  already supported by the strict cast path.
- `USING` culture clauses and unsupported target types remain rejected.
- `TRY_PARSE` retains its separate NULL-on-error contract.
- No locale-sensitive, CLR-backed, or cross-source behavior is implied.
