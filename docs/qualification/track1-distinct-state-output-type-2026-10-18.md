# Track 1 — typed exact COUNT DISTINCT output boundary — 2026-10-18

## Scope

Typed grouped aggregate batches carry output-type metadata alongside compact state bytes. The grouped-state decoders now validate every decoded accumulator against that metadata before accepting it. This closes a routing boundary where a valid exact `COUNT(DISTINCT ...)` payload could otherwise be accepted under an incompatible declared result type and fail later during result materialization.

Both in-memory Arrow batch decoding and the persisted Arrow stream decoder apply the same check. Batches without output metadata retain the compatibility path used by older partial producers.

## Evidence

Command:

```text
cargo test -p kaveon-exec typed_count_distinct_exchange_validates_output_type_and_unions_partials -- --nocapture
```

Result: 1/1 passed.

The regression creates two typed partials for the same group, with overlapping exact distinct values, decodes both through the typed exchange boundary, merges them, and verifies the exact union count is three. It then attaches an incompatible `DOUBLE` output declaration to the same `COUNT DISTINCT` state and verifies decoding fails closed with `aggregate state/output type mismatch`.

## Boundary

This qualifies typed exact distinct state acceptance and output-type validation at the exchange decoder. It does not change the legacy scalar aggregate merge contract, which continues to decline exact distinct plans.
