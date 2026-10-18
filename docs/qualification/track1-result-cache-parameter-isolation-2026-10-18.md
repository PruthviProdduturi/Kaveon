# Track 1 result-cache parameter isolation qualification

## Scope

Result-cache keys now include a stable SHA-256 digest of positional and named
bound parameters. Positional order remains significant; named-parameter map
order is canonicalized; nested arrays and objects remain value-sensitive. The
values themselves are never placed in telemetry or query records.

## Evidence

- Commit: `d287600b`
- `cargo test -p kaveon-server result_cache::tests --quiet`: 14 passed, 0 failed
- `cargo clippy -p kaveon-server --bin kaveon-server --tests -- -D warnings`: passed
- `cargo fmt --all -- --check`: passed
- `git diff --check`: passed

This closes parameter cross-contamination for the cache key. End-to-end cache
hit latency, revision invalidation, and production-shaped workload measurement
remain open.
