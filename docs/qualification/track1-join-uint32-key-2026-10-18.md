# Distributed join UInt32 key boundary

Date: 2026-10-18

Hash join key extraction now accepts `UInt32` keys and widens them losslessly
to the existing unsigned 64-bit hash representation. The focused executor
regression joins `u32::MAX`, a non-matching key, and NULL, proving that the
maximum unsigned value matches without signed narrowing and that NULL does not
match.

```text
cargo test -p kaveon-exec uint32_join_keys_match_without_signed_narrowing -- --nocapture
PASS (1 test)
```

The change is limited to UInt32 equality-key extraction; broader join costing,
skew, and cloud qualification remain separate boundaries.
