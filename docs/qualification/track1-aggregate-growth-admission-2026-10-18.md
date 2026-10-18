# Track 1 — aggregate growth admission qualification

## Scope

Columnar aggregate index-growth admission now uses saturating batch-size
arithmetic and an overflow-safe large-table growth threshold. A malformed or
hostile input length can no longer wrap the required-group calculation or the
load-factor comparison and bypass the reservation decision. Normal growth
policy is unchanged: larger integer-key tables may use a bounded fourfold
growth, while other shapes use doubling.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec \
  columnar_aggregate::tests::growth_bytes_covers_the_largest_index_growth_a_batch_can_cause \
  -- --nocapture
test result: ok. 1 passed; 0 failed
```

The regression retains the existing exact one-, two-, and fourfold growth
expectations and also asks the admission calculation for `usize::MAX` incoming
rows. It completes without overflow and returns a positive reservation.

## Boundary

This is a fail-closed arithmetic and admission guard. It does not change the
aggregate hash layout or claim a high-cardinality throughput improvement; the
ClickBench high-cardinality path still requires broader vectorized hashing and
exchange-volume work.
