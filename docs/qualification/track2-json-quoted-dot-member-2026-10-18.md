# Track 2 SQL/JSON quoted-dot member qualification

## Scope

SQL/JSON paths may quote an object member after a dot when the member name
contains punctuation. Kaveon now accepts paths such as `$."a.b".value` and
escaped quoted names, while preserving the existing bracket-quoted path rules.

## Evidence

- Commit: `e6dc6c00`
- Focused executor JSON tests: 19 passed, 0 failed
- `cargo fmt --all -- --check`: passed
- `git diff --check`: passed
- Differential corpus case: `json_quoted_dot_member`

This is a bounded path-compatibility improvement. Full SQL/JSON coercion,
complete path semantics, and broad external-client qualification remain open.
