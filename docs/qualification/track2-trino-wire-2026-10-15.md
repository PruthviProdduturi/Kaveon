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

## AKS image boundary

The currently deployed AKS digest
`sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
answered the distributed SELECT query correctly, but control/prepared requests
followed the older native statement behavior (`USE` was rejected as
SELECT-only and the prepare header was absent). Therefore the AKS wire gate is
not passed for that image. The image must be rebuilt from the current source
and redeployed before claiming cloud Trino-client compatibility.

This is an image qualification failure, not evidence that the current source
implementation is missing the lifecycle; the clean-source wire run and server
unit coverage both pass.
