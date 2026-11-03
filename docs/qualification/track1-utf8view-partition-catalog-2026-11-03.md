# Qualification: view-backed text in partition catalog metadata

Date: 2026-11-03  
Scope: Engine catalog partition-column type normalization

## Contract

Hive-style partition values are logical text. A source schema that reports a
partition column as Arrow `Utf8View`, or as a dictionary whose value type is
`Utf8View`, must be accepted by the catalog as the same `Utf8` partition
contract. The view representation must not make an otherwise valid partition
declaration fail closed.

## Evidence

- `cargo test -p kaveon-core partition_columns_treat_utf8_view_as_logical_text`
  — passes for direct and dictionary-encoded `Utf8View`, including catalog
  acceptance against the normalized `Utf8` partition definition.
- `cargo clippy -p kaveon-core --all-targets -- -D warnings` — passes.
- `cargo fmt --all -- --check` — passes.

## Boundary

This qualifies catalog type normalization for text partition metadata. It does
not claim that arbitrary nested partition types or unsupported partition
layouts are accepted.
