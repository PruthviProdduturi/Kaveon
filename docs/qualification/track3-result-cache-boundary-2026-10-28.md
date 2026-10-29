# Result-cache boundary qualification — 2026-10-28

## Status

Qualified as an analytical execution cache. This evidence does not treat
Parquet/Delta data caches or statistics answers as result-cache hits.

## Contract

- Cache identity includes normalized SQL, bound parameters, catalog snapshot,
  pinned source versions, and time zone.
- A hit reports `execution.mode = "cache"` and `execution.detail = "hit"`;
  it does not start coordinator or worker execution.
- `settings.result_cache = false` and `SET SESSION result_cache = false`
  bypass lookup and insertion for that statement.
- Paged results are not inserted into the complete-result cache.
- Entries are bounded by TTL, byte budget, and LRU eviction.
- Catalog publication and KaveonDB product commit/replay clear affected cached
  results; transactional revision/CAS state remains authoritative.
- Only an administrator can clear the cache through `DELETE /v1/cache`.

## Evidence

The server test
`api::tests::a_repeated_statement_is_served_from_the_result_cache` verifies:

1. the first execution records a miss and stores one complete result;
2. whitespace-normalized equivalent SQL hits the same result;
3. a changed literal misses;
4. per-statement and session bypasses do not add entries;
5. paged statements are excluded;
6. hit telemetry reports zero execution time and no stages;
7. non-admin cache clearing is forbidden, admin clearing succeeds; and
8. catalog publication invalidates the remaining entry.

The related product-transaction tests verify cache invalidation after committed
product state changes. The focused server test and the full server suite passed
on 2026-10-28; the SQL compatibility tracker records this as a bounded
qualification, not a claim about storage-level data caching.
