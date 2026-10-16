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

## Current-image rerun

The same qualification was rerun on the live `kaveon-aks` cluster against
Engine image
`kaveonacr.azurecr.io/kaveon-engine@sha256:0ea4244c392267f5d821935cb63f4c2a251c30be2f44a33554b340e1eb63a1f6`.
All three workers reported the pinned catalog snapshot
`sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`.
The three shapes again finished twice with exact repeated results: `EU=12,
NA=12` for the left outer self-join, `29` rows for the full outer
cross-source join, and `EU=144, NA=144` for the skewed grouped join. The raw
rerun is retained in
[`track1-aks-join-gates-rerun-2026-10-15.json`](./track1-aks-join-gates-rerun-2026-10-15.json).

This strengthens the bounded current-image equivalence evidence. It does not
close disk-exhaustion behavior, forced join-spill recovery, or larger
high-cardinality joins.

## Boundary

This qualifies bounded distributed outer-join equivalence and skewed join
execution on the declared three-worker AKS topology. It does not yet qualify
disk-exhaustion behavior, forced join-spill recovery, larger high-cardinality
joins, or the full multi-join cost policy; those remain open in the tracker.

## Forced-spill probe (negative evidence, 2026-10-15)

A low-memory probe against the current 504M-row `OpenSource.public.kaveon_events_enriched` table used a 1 MiB query budget, one local worker thread, disabled result caching and statistics, and a bounded `user_id`/date predicate. The request did not finish within the 300-second client window. The query was then canceled through `DELETE /v1/query/{id}`; its history settled to `CANCELED`, all stages stopped, all three workers remained catalog-compatible, and coordinator and worker queue depths returned to zero. No worker restarted.

The probe recorded zero `join_spill_bytes_written`, zero spill partitions, and zero spill depth, so this is **not** a forced join-spill qualification. It is retained as negative evidence and confirms cancellation cleanup. The next gate needs a purpose-built high-cardinality fixture or a prunable snapshot that completes within the qualification window and records exact spill/recovery counters.

Raw evidence: [`track1-aks-join-spill-negative-2026-10-15.json`](./track1-aks-join-spill-negative-2026-10-15.json).

## Forced join-spill qualification (bounded, 2026-10-15)

The current three-worker AKS image was run twice with a 1 MiB per-query budget, one local worker thread, disabled result caching and statistics, and a prunable date plus `user_id` range against `OpenSource.public.kaveon_events_enriched`. Both runs finished successfully with the same ordered six-row result. Each worker wrote 32 join-spill partitions (about 1.04 MiB of join spill per worker) and completed the final merge. A follow-up check found zero spill files on all three workers, and all three workers remained catalog-compatible with no restarts.

This qualifies bounded forced join spill, exact repeatability, and post-query cleanup. It does not qualify disk exhaustion, spill recovery after worker loss, larger high-cardinality joins, or sustained mixed-workload endurance; those remain open in the tracker.

Raw evidence: [`track1-aks-join-spill-qualification-2026-10-15.json`](./track1-aks-join-spill-qualification-2026-10-15.json).

## Forced join-spill worker-loss qualification (bounded, 2026-10-15)

The same 1 MiB spill query was observed running on the current three-worker AKS image while one StatefulSet worker was forcibly deleted. The replacement worker re-registered with the same catalog snapshot. The retried query finished with the exact baseline six-row ordered result and recorded 32 join-spill partitions on each worker before the loss. The post-recovery cluster reported three active and three catalog-compatible workers; live spill directories were empty afterward.

This qualifies bounded spill recovery across one worker loss. Disk exhaustion, larger high-cardinality joins, and sustained mixed-workload endurance remain open.

Raw evidence: [`track1-aks-join-spill-worker-loss-2026-10-15.json`](./track1-aks-join-spill-worker-loss-2026-10-15.json).


## Distributed aggregate spill qualification (bounded, 2026-10-15)

The current three-worker AKS image ran an ordered 10,000-group aggregation twice with a 4 MiB per-query budget, one local worker thread, disabled result caching and statistics, and a prunable date plus `user_id` range. Both runs returned the same 10,000 rows. The final merge wrote three spill runs totaling 164,184 bytes, and all three workers reported zero retained spill files afterward.

This qualifies bounded distributed aggregate spill, exact repeatability, and cleanup. Larger-scale AKS pressure, skew behavior, published p95 latency, disk exhaustion, and sustained mixed-workload endurance remain open.

Raw evidence: [`track1-aks-aggregate-spill-2026-10-15.json`](./track1-aks-aggregate-spill-2026-10-15.json).

