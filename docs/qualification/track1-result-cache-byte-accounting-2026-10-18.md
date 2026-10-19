# Result-cache byte-accounting qualification

## Result

Result-cache admission now uses saturating byte arithmetic for both the
budget comparison and the committed byte counter. If accounting ever reaches
an extreme `u64` value, the cache evicts before admitting the new result
instead of wrapping below its configured budget. This is a fail-closed memory
invariant; normal entries remain bounded by the one-eighth per-entry ceiling.

## Evidence

`result_cache::tests::near_maximum_accounting_evicts_before_byte_counter_can_wrap`
seeds the cache at a near-maximum accounting value and verifies that the
least-recent entry is evicted, the counter remains bounded, and only the new
result is served.

## Boundary

This is coordinator-local cache accounting. It does not claim distributed
cache coherence, persistent cache recovery, or deployed hit-latency results;
the cache key and catalog-publish invalidation continue to govern those
separate boundaries.