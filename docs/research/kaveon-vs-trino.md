# Kaveon and Trino: Architectural Comparison

> Verified September 4, 2026. This is an engineering comparison, not a benchmark claim. Kaveon states are taken from `dev`; Trino behavior is referenced to the current Trino 483 documentation.

## Executive position

Trino is the mature reference for distributed SQL federation. Kaveon is building a narrower integrated data-intelligence platform: a native Rust analytical engine, deterministic Data Language Model (DLM), and BI Studio delivered as one product. They overlap at distributed analytical SQL, but their product boundaries and maturity are different.

Kaveon should not claim to outperform Trino until identical-data, identical-hardware, concurrency-controlled benchmarks demonstrate it. The near-term engineering objective is more precise: match the correctness and operational properties required for lakehouse SQL while reducing layers between customer-owned data, deterministic semantic context, and visualization.

## System boundary

| Dimension | Kaveon `dev` | Trino 483 |
|---|---|---|
| Product boundary | Engine + deterministic DLM + Studio | Distributed SQL query engine |
| Runtime | Rust, Arrow `RecordBatch`, Axum control plane | JVM coordinator and workers |
| Primary data path | Direct local Parquet and Delta reads | Connector-defined access to many systems |
| Catalog | Native durable single-coordinator catalog; external adapters are contracts only | Connector catalogs, commonly backed by external metastores/catalog services |
| Query entry | Remote-first CLI and HTTP coordinator | CLI, JDBC, HTTP protocol, and ecosystem clients |
| BI experience | Native Kaveon Studio | External BI tools |
| NL→SQL | Deterministic DLM; no hosted LLM required | Outside Trino's engine boundary |
| SQL surface | Focused alpha subset; no CTEs, subqueries, HAVING, general `SELECT DISTINCT`, window functions, or `CASE` | Substantially broader production SQL surface, including these query families |

## Distributed execution

Both systems decompose a query into stages, tasks, splits, operators, and exchanges. Trino has a long-established production implementation: its coordinator plans and schedules work; workers fetch connector splits and exchange intermediate data. Kaveon now has the corresponding foundational contracts and an executable alpha path:

- versioned executable fragments;
- coordinator dependency-gated stage scheduling;
- deterministic Parquet row-group and Delta active-file splits;
- Arrow IPC exchange with bounded payload accounting and authentication;
- hash, single, round-robin, and broadcast partitioning;
- partial/final aggregates, distributed Sort/TopN, and repartitioned/broadcast joins;
- task attempts, alternate-worker retry, cancellation propagation, and cleanup.

Kaveon's path is not yet equivalent to Trino's operational maturity. Admission control, aggregate/join spill, durable spooled exchange, live task metrics, autoscaling evidence, and sustained failure testing remain gates. Trino's fault-tolerant execution can spool exchange data and retry queries or tasks when enabled; it is deliberately configurable and connector-dependent.

## Optimization and storage

| Area | Kaveon `dev` | Trino 483 |
|---|---|---|
| Projection pruning | Implemented | Connector/optimizer dependent and mature |
| Static filter pushdown | Implemented conservatively with residual evaluation | Broad connector pushdown framework |
| Parquet row-group pruning | Implemented | Supported by relevant connectors |
| Dynamic filtering | Not implemented | Runtime join filters can reach scans and split enumeration |
| Cost-based optimization | Not implemented | Join enumeration/distribution use connector statistics |
| Delta protocol | JSON history from version 0; no checkpoints/deletion vectors/time travel | Mature Delta connector with a broader protocol surface |
| Cloud object storage | ADLS Gen2/S3 target | Mature object-storage support through connectors |

Kaveon's advantage is potential control over a compact native hot path. Trino's advantage is the breadth and operational learning encoded in its optimizer, connectors, security model, and production deployments. Rust alone is not a performance result.

## Governance and security

Kaveon's platform uses Entra identity at the Studio/API boundary. Engine internal exchange and catalog mutations have dedicated bearer-token contracts, but end-user Engine authentication, authorization, TLS, resource groups, and quotas are incomplete. Catalog definitions persist credential references rather than secrets.

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
