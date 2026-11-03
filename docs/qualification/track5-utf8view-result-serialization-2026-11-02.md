# `Utf8View` result serialization qualification — 2026-11-02

## Contract

Engine result serialization emits Arrow `Utf8View` values as ordinary JSON
strings in both result paths:

- batched statement rows serialized by `batches_to_json`; and
- scalar/nested values serialized by `array_value_to_json`.
- statistics values and client-facing type labels treat `Utf8View` as text.
- the embedded/local CLI row adapter and table renderer treat `Utf8View` as
  text rather than falling back to Arrow's debug representation.
- map entries whose keys are `Utf8View` preserve the logical string key in the
  JSON object shape, so chart dimensions and key-based consumers do not lose
  matches at the API boundary.

This keeps text dimensions stable at the API boundary. Charts receive their
actual category labels, and clients that key or join result values do not see
Arrow debug representations such as `StringViewArray(...)`. Null handling and
all other data-type serialization behavior are unchanged.

The execution aggregate path also accepts `Utf8View` grouping keys and text
extrema, normalizing finalized output to the Engine's stable logical `Utf8`
type. This covers local grouped execution and the key preparation used before
distributed partial aggregation.

## Evidence

- Implementation commit: `fb52c9b1` (`fix(api): serialize utf8 view results as text`).
- `streamed_root_tests::result_rows_encode_utf8_view_values_as_text`
- `streamed_root_tests::result_rows_encode_utf8_view_values_as_text` also
  covers a `Map<Utf8View, Int64>` result and asserts the logical text key.
- `api::tests::statistics_statements_read_the_stored_statistics`
- `streamed_root_tests::presented_type_uses_client_facing_nested_names`
- `aggregate::tests::utf8_view_min_max_and_group_keys_use_logical_text_values`
- `local::rows::tests::utf8_view_rows_use_text_values`
- `local::display::tests::utf8_view_cells_render_as_text`
- `cargo test -p kaveon-server --bin kaveon-server utf8_view -- --nocapture` — passed (serializer and grouped-text regression tests)
- `cargo clippy -p kaveon-server --all-targets --no-default-features -- -D warnings` — passed
- `cargo test -p kaveon-cli` — 214 passed, 1 ignored

## Boundary

This is an API/result-rendering correctness fix; it does not alter query
planning, execution, storage, or the existing fail-closed behavior for
unsupported types. End-to-end chart qualification should use a text-dimension
dataset and verify the returned labels after the containing Engine image is
deployed.
