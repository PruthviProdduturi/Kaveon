# Track 1 worker scheduling: AKS qualification

**Date:** 2026-10-14  
**Cluster:** `kaveon-aks` in `kaveon-rg`, West US 2  
**Purpose:** production-topology evidence for worker discovery, split assignment,
stage dependencies, and exact distributed aggregation.

## Runtime

- Subscription: Visual Studio Enterprise Subscription
  (`4ed07f02-b111-4eea-98ce-1c177d573a51`)
- Kubernetes: `1.35.7`
- Nodes: 1 system node and 3 worker nodes, all `Ready`
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Coordinator and workers: Engine `0.1.0`
- Cluster endpoint reported `total_nodes=4`, `active_workers=3`, and
  `compatible_workers=3`.
- Coordinator and all workers reported catalog snapshot
  `sha256:e746af39a56b702aecb0de18b2b8c60d6d65bd774e2dbcd4b0b52631e19520d1`.

## Query

The query was submitted through the coordinator Service over a local
`kubectl port-forward` with the cluster catalog token held in the existing
Kubernetes Secret. The query disabled the result cache and statistics-answer
shortcuts so that it exercised the distributed read path:

```sql
SELECT platform, COUNT(*) AS n
FROM public.kaveon_events_enriched
GROUP BY platform
ORDER BY platform
```

Catalog: `OpenSource`  
Schema: `public`  
Settings: `result_cache=false`, `use_statistics=false`

## Result and scheduling evidence

- Query ID: `2453cd55-97d5-466f-8ce2-84d0816dac3b`
- State: `FINISHED`
- Execution mode: `distributed`
- Elapsed: `42,657 ms`
- Result rows:

  | platform | count |
  |---|---:|
  | Desktop | 189,018,984 |
  | Mobile | 157,494,792 |
  | Web | 157,486,224 |

  The counts sum to exactly `504,000,000`.

- Stage topology: stage 0 had 3 tasks, stage 1 had 3 tasks, and stage 2 had
  1 final task.
- Stage 0 and stage 1 assigned completed tasks to all three workers.
- Stage 2 performed the final merge on worker 0.
- The query completed without an error, retry, or partial result.

This verifies production worker discovery, three-way partition assignment,
exchange stage dependency, final merge, and exact result correctness on the
504-million-row Delta workload. It does not by itself close retry/lease,
skew, cancellation cleanup, worker-loss, or sustained-soak gates.
