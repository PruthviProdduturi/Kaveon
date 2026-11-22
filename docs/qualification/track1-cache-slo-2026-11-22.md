# Local result-cache qualification — 2026-11-22

## Result

**PASS for the local Docker coordinator.** The cache harness completed every
check against `http://127.0.0.1:8081` for `SELECT 1`:

- exact result parity across uncached, cold-cache, warm-cache, and bypass runs;
- 20 warm hits observed without a bypass hit;
- cache clear removed all entries and bytes;
- hit-latency metrics were present (warm p50 **3.269 ms**, p95 **3.600 ms**,
  p99 **4.093 ms**);
- after a real `docker restart kaveon-engine-coordinator`, the cache was empty
  (`entries=0`, `bytes=0`, `hits=0`), proving restart miss behavior.

The implementation unit suite also passed **18/18**, covering paged-result
exclusion, authorization/snapshot/source identity isolation, parameter
binding, TTL expiry, byte-bounded LRU eviction, housekeeping, clear behavior,
and disabled-cache semantics.

The complete machine-readable report is retained locally at
`tmp/cache-slo-local-20261122/report.json`; the restart observation is recorded
in `tmp/cache-slo-local-20261122/restart-evidence.json`.

No cloud resources were provisioned or accessed; incremental cloud spend was
**$0**.

## Boundary

This qualifies the declared local Docker coordinator and one small query. It
does not qualify cache behavior across an AKS rollout, multiple coordinators,
or a publication-scale workload. Those remain separate cloud/release gates.
