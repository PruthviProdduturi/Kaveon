# `Utf8View` result serialization qualification — 2026-11-02

## Contract

Engine result serialization emits Arrow `Utf8View` values as ordinary JSON
strings in both result paths:

- batched statement rows serialized by `batches_to_json`; and
- scalar/nested values serialized by `array_value_to_json`.
- statistics values and client-facing type labels treat `Utf8View` as text.

This keeps text dimensions stable at the API boundary. Charts receive their
actual category labels, and clients that key or join result values do not see
Arrow debug representations such as `StringViewArray(...)`. Null handling and
all other data-type serialization behavior are unchanged.

## Evidence

- `streamed_root_tests::result_rows_encode_utf8_view_values_as_text`
- `api::tests::statistics_statements_read_the_stored_statistics`
- `streamed_root_tests::presented_type_uses_client_facing_nested_names`
- `cargo test -p kaveon-server result_rows_encode_utf8_view_values_as_text --all-targets --no-default-features -- --nocapture` — passed
- `cargo clippy -p kaveon-server --all-targets --no-default-features -- -D warnings` — passed

## Boundary

This is an API/result-rendering correctness fix; it does not alter query
planning, execution, storage, or the existing fail-closed behavior for
unsupported types. End-to-end chart qualification should use a text-dimension
dataset and verify the returned labels after the containing Engine image is
deployed.
