# Kaveon Intelligence evaluation program

Kaveon Intelligence is the governed intelligence experience above Kaveon’s three pillars:
the deterministic Data Language Model (DLM), Kaveon Engine, and Kaveon Studio.
This plan evaluates measurable outcomes commonly expected from Fabric IQ-style
experiences without claiming that the products have identical scope.

The comparison is evidence-first. A capability passes only when a local test
produces a saved report, the result is reproducible, and the report records the
dataset, image, configuration, and identity. Unknown is not a pass.

## Evaluation gates

| Gate | Measure | Passing evidence |
| --- | --- | --- |
| Context grounding | Dataset, table, metric, dimension, join, and filter resolution | Every answer records the resolved context version; unsupported entities are rejected rather than invented. |
| Determinism | Normalized request, route, SQL, context, and result | Repeated input produces identical route, SQL digest, context digest, and result digest. |
| Freshness | Source changes, expiry, invalidation, and rebuild | Stale context is marked and routed explicitly; rebuild produces a receipt with source and version. |
| Identity and RBAC | Visibility by user, role, catalog, schema, and table | An unauthorized principal cannot discover names, values, SQL, cached results, or chart data. |
| Auditability | Question, route, SQL, source, context, result, and timings | A durable query record links caller, entry point, evidence, timings, and result digest. |
| Explainability | Why this dataset, route, and answer were selected | The response exposes source, freshness, route, bounded SQL, and reproducible evidence. |
| Execution quality | Exact results, failures, cancellation, and distributed behavior | Corpus results match expected values; failures are explicit; no silent partial result or fallback. |
| Latency | Context, cache, and live-query p50/p95 under a fixed workload | Each path has a recorded p50/p95 and resource profile; cold and warm runs are not mixed. |
| Cost and isolation | CPU, memory, bytes, spills, model calls, and external dependencies | Resource limits and counters are recorded; the deterministic DLM path has no hosted-model call. |
| Product continuity | Chat → SQL → chart → dashboard → CLI | Identity, source, permissions, and evidence remain consistent across surfaces. |

## Comparison guardrails

Do not claim parity with every Microsoft IQ, Fabric, Work IQ, or Foundry IQ
capability. Compare outcomes and disclose scope. Kaveon’s intended strengths are
deterministic dataset semantics, evidence-bearing SQL and context versions,
customer-owned storage and execution, a question-to-SQL/chart/dashboard path,
transactional KaveonDB product state, and no hosted model call on the DLM path.

The report must include the workload, data volume, engine image, hardware,
cache state, concurrency, identity, and failure behavior. A faster isolated
query is not evidence of overall superiority; a result that differs is not a
performance win.

## Docker-first local qualification

Run these locally before using any cloud environment. Use the actual portal
port exposed by the local stack; `3000` below is an example.

```powershell
docker compose -f docker-compose.kavedb.yml up -d
docker compose -f engine/qualification/compose.yml up -d --wait
docker build -t kaveon-engine:iq-local -f engine/Dockerfile engine
```

Run the DLM corpus and save its evidence:

```powershell
python scripts/qualify-dlm-questions.py `
  --portal http://127.0.0.1:3000 `
  --corpus docs/qualification/dlm-question-corpus.json `
  --execute-live `
  --report tmp/kaveon-iq-dlm-local.json
```

Run the engine qualification suite with a repeatable local workload:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:iq-local `
  --suite extended `
  --rows 1000000 `
  --customers 100000 `
  --warmups 1 `
  --repetitions 3 `
  --workers 2 `
  --output tmp/kaveon-iq-engine-local
```

Retain both reports and record the commit, image digest, host, worker count,
input files, cache state, and identity. Copy reports before teardown:

```powershell
docker compose -f engine/qualification/compose.yml down -v
docker compose -f docker-compose.kavedb.yml down -v
```

## Required evidence checks

For each representative question, verify all five items:

1. The resolved dataset and context version are present.
2. The route explicitly says context, cache, or live execution.
3. SQL is bounded, reproducible, and tied to the selected source.
4. The query record includes caller, source, timings, and result digest.
5. A denied role cannot retrieve the same metadata, result, cache, or chart data.

Also test context invalidation after a source change, cancellation cleanup,
worker failure behavior, replay of the same request, and an unsupported
question. The expected result for unsupported or unauthorized work is a clear
denial with no data leakage.

## Local release gate

Kaveon Intelligence is locally qualified only when the DLM corpus has exact expected
routes and evidence, deterministic replay has identical digests, freshness
invalidation has a rebuild receipt, RBAC negative tests show no leakage,
cancellation leaves no retained result/exchange files, engine exact-result and
failure tests pass in Docker, and p50/p95/resource counters are recorded for
context and live SQL. Product copy must continue to distinguish implemented,
qualified, and planned capabilities.

Cloud deployment is a later reproduction of these contracts, not the place to
discover them. This document deliberately makes no Azure, pricing, or
Fabric-equivalence claim without a saved local measurement.
