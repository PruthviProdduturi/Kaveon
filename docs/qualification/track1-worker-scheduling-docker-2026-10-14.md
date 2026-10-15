# Track 1 worker scheduling: Docker qualification

**Date:** 2026-10-14  
**Purpose:** local container evidence for Track 1, worker discovery and scheduling.  
**Scope:** reproducible local gate; AKS qualification remains separate.

## Runtime

- Docker Engine: `29.8.2`
- Coordinator image: `engine-coordinator:latest`, image ID
  `sha256:3c8b2c86a8b139395c1d0a5e409204f0f4ae4d7c8c37dd33caaeb0e73d4ee6ab`
- Worker 1 image: `engine-worker-1:latest`, image ID
  `sha256:a9f186d3bc2052f84c4e730a0cf006afccf25177f0864e68602424235c089f90`
- Worker 2 image: `engine-worker-2:latest`, image ID
  `sha256:e0972a3f5428e85706c521420250a00405bcfc3365d8e362563c311457196149`
- Source checkout at run: `a88bb2a1040724a196631a8014436dc2e4742f01`

The Compose topology was one coordinator plus two workers. `/v1/cluster`
reported `total_nodes=3`, `active_workers=2`, and `compatible_workers=2`.
Coordinator and both workers reported the same catalog snapshot:
`sha256:bb2bfa38084dc889265bdb1cfd14685706a83f58f6cae4008bd21710ccbdfda7`.

## Fixture and command

The fixture is a 10,000-row Parquet table with columns `id` and `group`,
17 group values, and 500-row row groups. Its SHA-256 is
`140ED27CF25423AA705A02BD09B7AACE7BC489C0DEC7A178C0BA10ACFC578FF6`.

The table was registered as `kaveon.default.track1_events`, then queried with:

```sql
SELECT group, COUNT(*) AS n
FROM track1_events
GROUP BY group
ORDER BY group
```

## Observed result

- Query ID: `a77b2581-dded-41af-a6a4-d4282fb52467`
- State: `FINISHED`
- Elapsed: `17 ms`
- Result rows: `17`
- Counts: groups `0` through `3` returned `589`; groups `4` through `16`
  returned `588`; total `10,000`.
- Execution: `distributed`, detail `fragments`.

Stage and assignment evidence from the query record:

| Stage | Tasks | Assignment | Evidence |
|---:|---:|---|---|
| 0 | 2 | partition 0 → `worker-1`; partition 1 → `worker-2` | each worker scanned 5,000 rows and produced 17 partial groups |
| 1 | 2 | `worker-1`, `worker-2` | both workers decoded exchange input and produced merge fragments |
| 2 | 1 | `worker-1` | final merge returned the ordered 17-row result |

Both workers completed their tasks. No spill, retry, stale attempt, or
abandoned-task condition occurred in this short correctness run. Those cases
remain open in the Track 1 gate and are covered separately by the scheduler
and runtime focused suites.

## Reproduction

From `engine/`:

```powershell
docker compose up -d --build
docker compose ps
Invoke-RestMethod http://127.0.0.1:8080/v1/cluster
```

Register the fixture and submit the query through `/v1/statement` using the
`kaveon` catalog and `default` schema. Retrieve the query record from
`/v1/query/{query_id}` and retain the stage/task mapping above.

This report does not close Track 1. AKS execution, retry/lease evidence, skew,
cancellation cleanup, and failure-path qualification are still required.
