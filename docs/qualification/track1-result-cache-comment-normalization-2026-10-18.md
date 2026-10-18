# Track 1 result-cache comment normalization — 2026-10-18

## Scope

The coordinator result-cache key now treats SQL line and block comments as
lexical whitespace outside quoted literals and identifiers. This makes
equivalent statements such as `SELECT /* note */ 1` and `SELECT 1` share a
cache key without allowing comment-like text inside `'...'` or `"..."` to
change meaning. A separator is retained around comments, so
`SELECT/**/1` normalizes to `select 1`, never `select1`.

## Evidence

From `engine/`:

```powershell
cargo fmt --all -- --check
cargo test -p kaveon-server result_cache --quiet
```

Result: 15 result-cache tests passed, including the comment-normalization
regression. The cache remains bounded by its configured byte budget and TTL;
this change only improves deterministic key equivalence.

## Boundary

This is a local coordinator unit qualification. It does not claim deployed
hit-latency, catalog invalidation, external Trino-client, or multi-coordinator
cache qualification; those remain open in the parity tracker.
