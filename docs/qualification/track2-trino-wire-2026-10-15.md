# Track 2 Trino wire qualification — 2026-10-15

## Clean current-source result

The clean build from the parity baseline was exercised through the raw
`/v1/trino/statement` route with Trino headers and paged result retrieval. The
following lifecycle passed:

1. `PREPARE p FROM SELECT CAST(? AS BIGINT) AS n`
2. `DESCRIBE INPUT p` — position 0, type `bigint`
3. `EXECUTE p USING 42` — exact result `[[42]]`
4. `DEALLOCATE PREPARE p`

The route emitted the expected `X-Trino-Added-Prepare` and
`X-Trino-Deallocated-Prepare` headers, and the execute response's `nextUri`
was drained successfully. The local driver is retained as
`tmp/local-trino-wire-current.py`.

## AKS current-image result

The current source image was rebuilt from commit `4199d496` and deployed to the
coordinator and all three workers at immutable digest
`sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`.
The raw external wire driver then passed every check:

- `USE OpenSource.public` returned `X-Trino-Set-Catalog` and
  `X-Trino-Set-Schema`.
- `PREPARE`, `DESCRIBE INPUT`, `EXECUTE ... USING 42`, and `DEALLOCATE PREPARE`
  returned the expected headers and exact typed result `[[42]]`.
- The distributed grouped query returned six exact region/count rows through
  paged `nextUri` retrieval.

The report is retained as
`tmp/aks-trino-wire-qualification-2026-10-15-fixed.json`.

The earlier digest `sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
was rejected as stale: it exposed the older SELECT-only behavior. During the
rollout, a second defect in projected object reads was found and fixed in
`4199d496`; the reader now translates full-schema column indexes into positions
in the physically ordered projected batch. The storage suite passed 121 tests
before the fixed image was deployed.
