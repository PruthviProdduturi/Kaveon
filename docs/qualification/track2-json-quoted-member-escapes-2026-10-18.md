# Track 2 SQL/JSON quoted member escape qualification

## Scope

Quoted SQL/JSON path member names now decode the standard short escapes and
Unicode `\uXXXX` escapes in both dot notation and bracket notation. This keeps
keys containing whitespace, punctuation, and non-ASCII characters addressable
without changing the existing fail-closed behavior for malformed paths.

## Evidence

- Focused executor JSON tests: 20 passed, 0 failed
- `cargo fmt --all -- --check`: passed
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings`: passed
- `git diff --check`: passed

This is bounded path compatibility evidence. Full SQL/JSON coercion, complete
path semantics, and external-client differential coverage remain open.
