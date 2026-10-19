# Track 1 result-cache source invalidation — 2026-10-18

## Invariant

A completed result may be reused only while every planner-pinned source identity remains unchanged. A changed source identity must miss the cache, while an unrelated entry remains available.

## Evidence

`kaveon-server` test `result_cache::tests::a_source_identity_change_misses_without_clearing_unrelated_entry` inserts a result, changes one pinned `source_versions` value, verifies the changed key misses, and verifies the original key still hits. This exercises behavioral invalidation rather than only comparing key structs.

## Boundary

This is coordinator-local result-cache behavior. It does not qualify distributed cache coherence, source identity publication latency, or deployed cloud storage notification behavior.
