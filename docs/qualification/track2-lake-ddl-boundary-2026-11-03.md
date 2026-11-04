# Track 2 — lake-table index and constraint DDL boundary — 2026-11-03

## Status

**Qualified fail-closed boundary.** This record does not claim that Kaveon
implements general SQL indexes or constraints for lake tables. It records that
unsupported statements are rejected before planning with an actionable error.

## Covered statements

The SQL parser rejects these forms explicitly:

- `CREATE INDEX`
- `CREATE UNIQUE INDEX`
- `DROP INDEX`
- `ALTER TABLE ... ADD CONSTRAINT`

The diagnostic distinguishes the unsupported lake-table DDL from physical
layout controls such as `clustered_by` and Bloom filters. Those controls affect
file organization and pruning; they are not uniqueness, foreign-key, or
transactional indexes.

## Evidence

The parser regression
`parser::tests::unsupported_lake_index_and_constraint_ddl_fails_closed`
covers each statement family, including `--` and `/* ... */` comment prefixes
and arbitrary SQL whitespace between leading keywords, and asserts the
actionable diagnostic. Near-miss identifiers such as `INDEXED_TABLE` remain
ordinary identifiers. The full SQL crate suite passed with 380 tests:

```text
cargo test -p kaveon-sql --lib
```

The original boundary is in commit `856ccc61`; comment-prefix hardening is in
`dad97d3c`, with keyword-whitespace hardening in the follow-up commit below.

## Promotion gate

These statements may only move from rejected to supported after catalog
revision metadata, writer enforcement, snapshot-replacement semantics,
atomic rollback, restart/replay recovery, and local plus distributed
qualification are implemented. Until then, accepting the syntax would imply
integrity semantics that Parquet/Delta/Iceberg layout metadata does not
provide.
