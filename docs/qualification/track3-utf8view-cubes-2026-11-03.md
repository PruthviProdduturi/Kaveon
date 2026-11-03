# Qualification: view-backed text in chart/dashboard cubes

Date: 2026-11-03  
Scope: Kaveon Engine cube materialization (`kaveon-storage`)

## Contract

Chart and dashboard cubes treat Arrow `Utf8View` columns as logical text
dimensions. A view-backed value must produce the same `StatValue::Text` key as
an equivalent `Utf8` value, including nulls and non-ASCII text. Cube readers
must not surface Arrow debug representations or silently reject the column.

## Implementation

`storage::cube::RowReader` normalizes `Utf8View` to `Utf8` at the reader
boundary before row-wise dimension extraction. This keeps the cube key path
consistent with the query result serializer, DISTINCT executor, fused
aggregation, and source-statistics paths.

## Evidence

- `cargo test -p kaveon-storage row_reader_normalizes_utf8_view_text_for_cube_dimensions`
  — passes; verifies normal text, null, and non-ASCII view values.
- `cargo clippy -p kaveon-storage --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.
- Existing storage cube build/refresh tests remain covered by the storage test
  suite; the new case isolates the view-backed dimension boundary.

## Boundary

This qualifies cube dimension extraction only. It does not claim that every
Arrow nested type, unsupported cube measure, or unqualified distributed
materialization mode is complete.
