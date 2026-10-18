# Track 1 exchange assembly buffer — 2026-10-18

## Boundary

The coordinator-side exchange assembler now reserves one exact payload buffer
after chunk validation and appends each ordered chunk into it. This removes
the repeated growth and copy cycle that previously occurred while assembling
large Arrow exchange payloads.

## Evidence

The focused server test assembles out-of-order two-chunk input, verifies the
exact byte sequence, and asserts that the resulting allocation capacity equals
the payload length:

```text
cargo test -p kaveon-server exchange::tests::assembly_reserves_one_exact_payload_buffer -- --nocapture
```

The existing duplicate, mixed-identity, missing-chunk, and payload-limit tests
continue to run against the same validation path.

## Remaining boundary

This is an allocation reduction for in-memory exchange assembly. It does not
claim a fully streaming coordinator result path or eliminate all exchange
serialization costs.
