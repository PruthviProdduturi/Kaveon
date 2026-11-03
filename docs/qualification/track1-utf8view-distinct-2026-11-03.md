# `Utf8View` distributed DISTINCT qualification — 2026-11-03

## Contract

The DISTINCT executor and columnar grouped-count path treat Arrow `Utf8View`
values as their logical UTF-8 text. View-backed values use the same
interned-key and canonical row-key paths as ordinary UTF-8 values, preserving
duplicate elimination, grouped counts, and NULL semantics across input
batches. This includes dictionary-encoded view-backed text.

## Evidence

- `distinct::tests::utf8_view_values_are_distinct_by_text_across_batches`
  covers repeated text across batches, a distinct second value, a NULL, and a
  later value.
- `columnar_aggregate::tests::dictionary_utf8_view_keys_fold_by_text` covers
  dictionary codes, repeated values, and NULL keys on the fused grouped-count
  path.
- `cargo test -p kaveon-exec --lib distinct::tests::utf8_view_values_are_distinct_by_text_across_batches` — passed
- `cargo test -p kaveon-exec --lib columnar_aggregate::tests::dictionary_utf8_view_keys_fold_by_text` — passed
- `cargo clippy -p kaveon-exec --all-targets -- -D warnings` — passed
- `cargo fmt --all -- --check` — passed

## Boundary

This qualifies exact DISTINCT key handling for `Utf8View`; it does not claim
new ordered-map semantics or broaden unsupported complex-type DISTINCT forms.
