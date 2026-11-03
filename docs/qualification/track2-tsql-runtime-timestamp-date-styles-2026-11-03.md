# Runtime T-SQL timestamp date-style qualification — 2026-11-03

`TRY_CONVERT(DATETIME2, text, style)` has exact local and three-worker
coverage for the bounded SQL Server date-only styles already implemented by
the runtime parser: `0`, `1`, `23`, `101`, `102`, `103`, `104`, `105`, `106`,
`107`, `110`, `111`, and `112`. ISO timestamp styles `20`, `21`, `120`, `121`,
and `126`, plus SQL Server style `113` (`dd mon yyyy hh:mi:ss:mmm`), remain
covered in the same differential. Style `113` is parsed with invariant English
month abbreviations and preserves millisecond precision in the Engine's
microsecond timestamp representation.

Date-only styles produce midnight `TIMESTAMP` values. Style `113` preserves its
time-of-day; malformed and NULL input
preserve `TRY_CONVERT` NULL behavior. This is an invariant parsing contract;
locale-specific date interpretation and unsupported style codes remain
rejected rather than guessed.

Evidence:

```text
cargo test -p kaveon-server --bin kaveon-server differential_tests::distributed_tsql_runtime_timestamp_styles_match_local -- --nocapture
```

The test compares the local planner/executor with the in-process three-worker
fragment path and asserts zero retained query-pool bytes after both runs.
