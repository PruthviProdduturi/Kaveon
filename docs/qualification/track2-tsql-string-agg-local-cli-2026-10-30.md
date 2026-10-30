# Local CLI `STRING_AGG` qualification — 2026-10-30

## Scope

The embedded/local CLI planner now uses the same bounded `STRING_AGG` executor
state as the server planner. This record covers the declared local surface:

- `STRING_AGG(value, literal_separator)` only;
- NULL input values are omitted;
- the separator must be a text literal no larger than 64 KiB;
- ordered `WITHIN GROUP`, `DISTINCT`, `FILTER`, and dynamic separators remain
  explicitly rejected.

This is local execution evidence. Distributed qualification remains the
server-side record in
`track2-tsql-string-agg-boundary-2026-10-30.md`.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kaveon-cli --all-targets
```

Results on the committed implementation:

- formatting passed;
- clippy passed with warnings denied;
- 213 CLI tests passed, including
  `local::tests::executes_bounded_string_agg_with_literal_separator`.

The regression executes `SELECT STRING_AGG(kind, '|') AS joined FROM events`
over the local Parquet fixture and verifies the exact result `click|view`, with
the NULL fixture row omitted and the output type preserved as UTF-8.

