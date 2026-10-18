# Track 2: Trino result-page link guard

**Date:** 2026-10-18`n**Scope:** Trino-compatible response shaping in the coordinator.

`trino_response_shape` now validates every native `next_uri` before exposing it
as Trino's `nextUri`. It accepts the native relative
`/v1/query/<id>/results/<page>` form and an absolute URL whose path has that
same result-page shape, then drops the authority and emits a relative
`/v1/trino/query/<id>/results/<page>` link. It rejects non-result routes,
non-numeric pages, query strings, fragments, malformed values, and non-string
`next_uri` values with a fail-closed internal error. This prevents a malformed
or compromised internal result record from handing an external Trino client a
link to an unrelated route or host.

Focused evidence:

- `trino_next_uri_accepts_only_result_pages_and_drops_authority`
- `trino_response_shape_rejects_unsafe_paging_links`
- `trino_response_shape_uses_camel_case_paging_and_stats`

This is a response-boundary hardening item. It does not claim broader external
client compatibility or change the result-page authorization/cancellation
contract; those remain open release-corpus work in the parity tracker.\n