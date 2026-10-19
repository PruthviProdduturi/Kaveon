# Track 1 — grouped spill layout preflight — 2026-10-18

## Scope

The columnar final aggregate now validates the compact state envelope and its first accumulator tag before resolving any new group slots. A malformed or incompatible spill/exchange row for a previously unseen key therefore fails before mutating the group table; retrying the batch cannot retain a half-created group. The same preflight runs for selected-row merges.

## Evidence

Command:

```text
cargo test -p kaveon-exec encoded_batch_merge_rejects_states_of_another_layout_or_malformed_bytes -- --nocapture
```

Result: 1/1 passed.

The regression sends an incompatible integer-SUM state for a new key into a COUNT columnar final merge and verifies the merge rejects it while the group table remains at its prior cardinality. Existing malformed row-count, truncated state, trailing key, and trailing state cases continue to fail closed in the same test.

## Boundary

This protects slot admission and retry behavior for the columnar grouped final path. It validates the envelope and first accumulator tag before admission; full payload validation remains in the existing merge parser and no claim is made for row-oriented aggregate fallback.
