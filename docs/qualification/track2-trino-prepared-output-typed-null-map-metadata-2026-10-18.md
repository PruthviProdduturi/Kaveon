# Track 2: prepared output metadata for typed NULL maps

## Scope

This qualification covers the prepared-output metadata path for a typed NULL
map expression:

```sql
SELECT CAST(NULL AS MAP(VARCHAR, BIGINT)) AS merged
```

The metadata contract reports the logical signature `map(varchar,bigint)` even
though the expression has no runtime value. This is the metadata required by a
Trino client before it fetches results.

## Evidence

The server unit test
`api::streamed_root_tests::trino_describe_output_infers_typed_null_map_metadata`
passed with:

- one output column named `merged`;
- output type `map(varchar,bigint)`;
- nullable output metadata;
- no runtime rows required to infer the type.

Command:

```text
cargo test -p kaveon-server trino_describe_output_infers_typed_null_map_metadata -- --nocapture
```

Result: 1 passed, 0 failed.

The corresponding local and three-worker value-path differential is recorded in
`track2-map-concat-null-input-2026-10-18.md`. This item qualifies metadata and
value behavior locally; external-client wire and cloud qualification remain
open.

## Boundary

This does not claim general nested-map cast support. The current bounded parser
accepts the supported scalar key/value targets used by this case; broader
recursive cast targets and external-client qualification remain open.
