# Kaveon and Trino: Architectural Comparison

> Written September 4, 2026 and revised September 17, 2026 against `dev` at `2dd11b7`. This is an engineering comparison, not a benchmark claim; the measured comparisons are the records under `docs/qualification/` (matched harness 1.45× over five rounds on 2026-09-15, scale suite ahead on 9 of 13 statements on 2026-09-16, ClickBench 1.39× geometric mean over 42 statements on the `8d15fd3` pass, TPC-H 21 of 22 planned and executed), none of which has completed the rounds `benchmark-program.md` requires for a published claim. Trino behavior is referenced to the Trino 483 documentation.

## Executive position

Trino is the mature reference for distributed SQL federation. Kaveon is building a narrower integrated data-intelligence platform: a native Rust analytical engine, deterministic Data Language Model (DLM), and BI Studio delivered as one product. They overlap at distributed analytical SQL, but their product boundaries and maturity are different.

Kaveon should not claim to outperform Trino until the identical-data, identical-hardware, concurrency-controlled campaign in `docs/qualification/benchmark-program.md` completes its rounds; the one-pass records so far show Kaveon ahead on scans, filters, TopN and low-cardinality aggregates and behind on the final merge over near-unique keys. The near-term engineering objective is more precise: match the correctness and operational properties required for lakehouse SQL while reducing layers between customer-owned data, deterministic semantic context, and visualization.

## System boundary

| Dimension | Kaveon `dev` | Trino 483 |
|---|---|---|
| Product boundary | Engine + deterministic DLM + Studio | Distributed SQL query engine |
| Runtime | Rust, Arrow `RecordBatch`, Axum control plane | JVM coordinator and workers |
| Primary data path | Direct Parquet, Delta (v1 checkpoints) and Iceberg reads on local disk and ADLS Gen2; S3 implemented, unqualified | Connector-defined access to many systems |
| Catalog | Native durable single-coordinator catalog; external adapters are contracts only | Connector catalogs, commonly backed by external metastores/catalog services |
| Query entry | Remote-first CLI, HTTP coordinator with per-request settings and `SET SESSION`, the API bridge for Studio | CLI, JDBC, HTTP protocol, and ecosystem clients |
| BI experience | Native Kaveon Studio | External BI tools |
| NL→SQL | Deterministic DLM; no hosted LLM required | Outside Trino's engine boundary |
| SQL surface | Joins, CTEs, derived tables, HAVING, DISTINCT, window functions with frames, set operations, CASE/LIKE/REGEXP, decorrelated EXISTS and scalar subqueries; TPC-H 21 of 22, every ClickBench statement. Refused by name: correlated non-equality subqueries, GROUPING SETS, approximate aggregates, array/map/JSON types (`docs/reference/engine-sql-compatibility.md`) | Substantially broader production SQL surface: lateral joins, GROUPING SETS, hundreds of scalar functions, table functions, prepared statements |

## Distributed execution

Both systems decompose a query into stages, tasks, splits, operators, and exchanges. Trino has a long-established production implementation: its coordinator plans and schedules work; workers fetch connector splits and exchange intermediate data. Kaveon now has the corresponding foundational contracts and an executable alpha path:

- versioned executable fragments with pinned Delta versions;
- coordinator dependency-gated stage scheduling and a FIFO memory admission queue;
- deterministic Parquet row-group and Delta active-file splits, parallel decoder lanes;
- Arrow IPC exchange, authenticated, streamed while the task runs, spooled on the consuming worker's disk;
- hash, single, round-robin, and broadcast partitioning;
- columnar partial/final aggregates that flush on pressure and spill sub-partitions on the final merge, distributed DISTINCT, Sort/TopN, repartitioned/broadcast joins, semi/anti joins and set operations;
- task attempts, alternate-worker retry (exercised with forced worker loss on AKS), cancellation propagation, orphan-task cleanup.

Kaveon's path is not yet equivalent to Trino's operational maturity. Streaming decode on the exchange consumer, spill qualified under skew, live per-operator metrics, autoscaling evidence, and sustained failure testing remain gates. Trino's fault-tolerant execution can spool exchange data and retry queries or tasks when enabled; it is deliberately configurable and connector-dependent.

## Optimization and storage

| Area | Kaveon `dev` | Trino 483 |
|---|---|---|
| Projection pruning | Implemented | Connector/optimizer dependent and mature |
| Static filter pushdown | Implemented conservatively with residual evaluation | Broad connector pushdown framework |
| Parquet row-group pruning | Implemented, including byte-array statistics writers mark inexact | Supported by relevant connectors |
| Dynamic filtering | Not implemented | Runtime join filters can reach scans and split enumeration |
| Cost-based optimization | Exact-statistics broadcast choice only; no join reordering | Join enumeration/distribution use connector statistics |
| Delta protocol | JSON commits and v1 checkpoints, pinned version; reader protocol v2 (column mapping, deletion vectors) and time travel refused | Mature Delta connector with a broader protocol surface |
| Cloud object storage | ADLS Gen2 with workload identity, qualified on AKS; S3 through the same reader, unqualified | Mature object-storage support through connectors |
| Result cache | Coordinator result cache keyed by statement, catalog snapshot and Delta versions | Not built in |

Kaveon's advantage is potential control over a compact native hot path. Trino's advantage is the breadth and operational learning encoded in its optimizer, connectors, security model, and production deployments. Rust alone is not a performance result.

## Governance and security

Kaveon's platform uses Entra identity at the Studio/API boundary. The Engine authenticates principals with roles, validates Entra bearer tokens, accepts delegated identity from the API bridge, serves native TLS, and applies per-principal limits, resource groups and memory admission; rotation without restart, tenant isolation, row/column policies and durable audit are open. Catalog definitions persist credential references rather than secrets.

Trino centralizes cluster access through the coordinator and supports pluggable authentication, system access control, column restrictions, row filters, column masking, secrets management, and secured internal communication. Kaveon must close these gaps before an enterprise comparison can be favorable.

## Where Kaveon is intentionally different

1. **One product surface.** Studio, DLM, and Engine share a product contract rather than requiring a separate BI product and semantic/NL layer.
2. **Deterministic language layer.** DLM compiles dataset context and routes supported questions without a hosted generative model.
3. **Customer-owned lake path.** The default architecture reads registered lake data in place; optimized ingest remains optional and writes back to customer-controlled storage.
4. **Native catalog direction.** Hive is not mandatory. Interoperability adapters remain optional boundaries.
5. **Evidence discipline.** Unsupported metrics and capabilities remain unavailable/target instead of being inferred.

## Decision guide

Choose Trino today when connector breadth, production-proven federation, mature security, cost-based planning, dynamic filtering, and established large-cluster operations are mandatory.

Evaluate Kaveon when the required sources are supported and the value comes from the integrated Engine + DLM + Studio workflow, deterministic conversational analytics, or control of a focused lakehouse execution path. Production adoption still requires workload-specific correctness, scale, failure, and security qualification.

## Required proof before comparative performance claims

- identical immutable Parquet/Delta snapshots and query results;
- identical compute, memory, storage, and network limits;
- documented warm/cold cache state and table statistics;
- single-user latency and controlled concurrency throughput;
- scan, join, aggregation, Sort/TopN, skew, spill, and worker-loss cases;
- p50, p95, variance, resource use, and cost per completed workload;
- published versions, configuration, SQL, harness, and raw results.

## Primary references

- [Trino concepts and distributed architecture](https://trino.io/docs/current/overview/concepts.html)
- [Trino dynamic filtering](https://trino.io/docs/current/admin/dynamic-filtering.html)
- [Trino cost-based optimization](https://trino.io/docs/current/optimizer/cost-based-optimizations.html)
- [Trino fault-tolerant execution](https://trino.io/docs/current/admin/fault-tolerant-execution.html)
- [Trino security overview](https://trino.io/docs/current/security/overview.html)
- Kaveon implementation truth: `ARCHITECTURE.md`, `STATUS.md`, `HANDSHAKE.md`, and `engine/DISTRIBUTED_EXECUTION_STATUS.md`
