# Track 1 exchange store insertion bounds qualification

## Scope

The in-memory exchange store now validates every directly constructed
`ExchangeChunk` with the same payload, chunk-count, chunk-index, and identity
limits used by the authenticated wire decoder. This closes a local insertion
path that could otherwise bypass exchange bounds before the store accounted for
the payload.

## Evidence

- `cargo fmt --all -- --check`: passed
- `cargo test -p kaveon-server --bin kaveon-server store_rejects_chunks_that_bypass_wire_decode_limits -- --nocapture`: 1 passed, 0 failed
- `cargo test -p kaveon-server --bin kaveon-server store_accepts_idempotent_retries_and_rejects_conflicts -- --nocapture`: 1 passed, 0 failed
- `git diff --check`: passed

This is local exchange-control-plane evidence. It does not claim a new AKS
topology, workload, or distributed execution SLO qualification.
