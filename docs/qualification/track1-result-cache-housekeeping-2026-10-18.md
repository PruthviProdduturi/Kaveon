# Result-cache housekeeping qualification — 2026-10-18

The coordinator now removes expired result-cache entries during its existing
30-second housekeeping cycle. Previously, expiry was enforced only when the
specific key was requested again, so a cold workload could leave expired rows
and their byte estimates occupying the cache budget until a later insertion.

The cleanup pass removes entries older than the configured TTL, updates LRU
byte accounting through the normal removal path, and increments the existing
expiration counter. Disabled caches return without taking the cache lock. The
focused `result_cache` suite passed 16 tests, including a regression that
expires an entry without a follow-up lookup and verifies zero retained entries
and bytes.

This is local coordinator evidence. It does not qualify deployed-image
hit-latency, catalog invalidation, or multi-coordinator cache coordination.
