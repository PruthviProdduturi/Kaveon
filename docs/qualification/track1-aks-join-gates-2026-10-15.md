# Track 1: AKS distributed join qualification

**Date:** 2026-10-15  
**Environment:** `kaveon-aks`, namespace `kaveon`  
**Topology:** three catalog-compatible workers  
**Catalog snapshot:** `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`

The bounded join probe ran each shape twice through the Engine API and required
both executions to finish with identical results. Every query created multiple
stages and tasks across all three workers.

| Shape | Result | Stages / tasks |
|---|---:|---:|
| Left outer self-join by `user_id`, grouped by region | `EU=12`, `NA=12` | 5 / 13 |
| Full outer cross-source join on `region = borough` | `29` rows | 4 / 10 |
| Skewed grouped self-join on `region` | `EU=144`, `NA=144` | 5 / 13 |

All repeated result digests matched, all responses were HTTP 200 and
`FINISHED`, and the cluster reported three active, catalog-compatible workers.
The skewed shape also retained exact results when one worker carried the
longest task, demonstrating deterministic completion under uneven work.

Raw evidence is retained in
[`track1-aks-join-gates-2026-10-15.json`](./track1-aks-join-gates-2026-10-15.json).

## Boundary

This qualifies bounded distributed outer-join equivalence and skewed join
execution on the declared three-worker AKS topology. It does not yet qualify
disk-exhaustion behavior, forced join-spill recovery, larger high-cardinality
joins, or the full multi-join cost policy; those remain open in the tracker.
