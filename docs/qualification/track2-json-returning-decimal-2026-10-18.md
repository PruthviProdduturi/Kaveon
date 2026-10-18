# Track 2 SQL/JSON decimal RETURNING qualification — 2026-10-18

Kaveon now accepts the bounded SQL/JSON form:

```sql
JSON_VALUE(document, path RETURNING DECIMAL(precision, scale))
```

The parser validates the requested precision and scale before lowering the
form to the existing exact Arrow `Decimal128` cast path. Precision is limited
to 1 through 38 and scale must not exceed precision. Unsupported or malformed
decimal bounds remain parser errors, so a wider Trino decimal is not silently
rounded into a narrower representation.

Coverage includes the SQL parser lowering, explicit invalid-bound rejection,
and exact local and three-worker distributed differential execution of
`JSON_VALUE(JSON_PARSE('{"amount":12.34}'), '$.amount' RETURNING DECIMAL(10, 2))`
across all registered Parquet layouts used by the differential corpus.

Evidence:

```text
cargo fmt --all -- --check                         pass
cargo test -p kaveon-sql json_returning --quiet    4 passed
cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths \
  -- --nocapture                                    1 passed, 0 failed (369.61 s)
```

This closes one bounded decimal coercion gap. Full SQL/JSON coercion,
complete path semantics, and broader external-client or cloud qualification
remain open in the Track 2 JSON row.
