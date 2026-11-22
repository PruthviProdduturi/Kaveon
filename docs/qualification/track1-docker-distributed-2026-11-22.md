# Local Docker distributed qualification — 2026-11-22

## Result

**PASS.** The release Docker image passed the extended two-worker same-file
correctness gate: 11/11 queries, zero mismatches, zero execution errors, and
distributed stages observed for the non-metadata workload.

This is a local release-artifact qualification. It does not certify AKS,
ADLS, node loss, rolling upgrades, or cloud object-store behavior.

## Reproduction

```powershell
docker compose -f engine/qualification/compose.yml up -d trino --wait
docker build -t kaveon-engine:qualification -f engine/Dockerfile engine
& engine/qualification/venv/Scripts/python.exe engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification `
  --suite extended --rows 500000 --customers 50000 `
  --warmups 1 --repetitions 2 --workers 2 `
  --output tmp/qualification-docker-2worker-20261122
```

The retained report is
`tmp/qualification-docker-2worker-20261122/report.json`.

## Evidence

| Item | Value |
|---|---|
| Git commit | `9bf769caaa3fc54804a755fb0f435add0726a0c8` |
| Kaveon image | `sha256:4ad4db2796a6437188da2234857e68edcdc91e6439c2f3b37983d3913863ed61` |
| Reference | Trino 483, image `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` |
| Matched resources | 4 CPUs / 8 GiB per service |
| Kaveon topology | coordinator + 2 workers; 2 distributed stages or more where required |
| Fixture | 500,000 event rows, 50,000 customer rows, two Snappy Parquet files in Delta v1 fixtures |
| Correctness | 11/11 exact result checks passed; floating reductions use the harness tolerance (`1e-12`) |
| Failures | 0 |
| Cloud resources | None provisioned or accessed; $0 incremental Azure spend |

Cases covered: filtered sum, grouped sum, join, small-left join, top-N,
distinct, unfiltered count, arithmetic projection, medium-cardinality groups,
high-cardinality groups, multi-aggregate, and grouped join.

The report's timing data is diagnostic only. This short two-worker run is a
correctness gate, not the publication-scale performance comparison.

## Remaining qualification boundary

The production-release evidence still needs the cloud-only gates when AKS and
ADLS are available: live object-store reads/writes and fault injection, worker
and coordinator failure recovery, rolling upgrade/rollback, sustained cloud
soak, multi-coordinator consistency, and the declared publication-scale
performance run. Until those are executed, the Engine should be described as a
locally qualified release candidate rather than universally production
certified.
