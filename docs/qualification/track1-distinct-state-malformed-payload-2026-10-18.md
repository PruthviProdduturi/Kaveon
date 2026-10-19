# Track 1 — typed distinct-state exchange fail-closed boundary

## Scope

The compact grouped-state envelope carries exact `COUNT(DISTINCT)` values as
typed, length-delimited payloads. This qualification verifies the decoder
rejects a malformed typed value and a payload length that exceeds the frame
before the state can enter the final aggregate merge.

## Evidence

From `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-exec compact_state --quiet
running 3 tests
3 passed; 0 failed
```

The focused regression mutates a valid one-value `COUNT(DISTINCT)` compact
state twice: once to replace the typed integer tag with an invalid NULL-tagged
payload, and once to declare a payload larger than the remaining frame. Both
forms fail closed without allocation or out-of-bounds reads.

## Boundary

This validates the typed compact-state transport boundary in-process. It does
not qualify object-store exchange, worker loss recovery, or an unbounded exact
distinct workload; those remain separate distributed qualification gates.
