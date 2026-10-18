# Decimal256 runtime-filter qualification — 2026-10-18

Decimal256 equality keys now use an exact typed 256-bit hash index in the join
build and probe paths. The runtime batch filter checks the original
`Decimal256Array` values directly and does not narrow them to Decimal128 or a
storage scalar. Duplicate keys and NULL probe values retain the existing join
semantics.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec decimal256_ -- --nocapture
```

Result: **3 passed**. The tests cover exact values outside Decimal128's range,
duplicate build keys, NULL handling, and the typed runtime-filter path.

This is local executor evidence. Decimal256 scale conversion, cloud storage
predicate pushdown, and deployed-image qualification remain open.
