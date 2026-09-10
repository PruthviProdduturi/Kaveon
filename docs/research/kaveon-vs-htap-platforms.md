# Kaveon and unified transactional/analytical platforms

> Verified September 10, 2026. Competitor behavior is referenced to the vendor primary sources cited at the end. Kaveon states are taken from `dev` at `c78878a`. This is a positioning and maturity assessment, not a benchmark claim and not a superiority claim.

## Executive position

Unifying transactional and analytical processing on cloud object storage is a contested market with at least four credible entrants, not an empty category. Kaveon must not describe its transactional direction as unprecedented. Two claims in particular fail immediately under scrutiny and must not be used:

1. **"No other engine does distributed and transactional directly on cloud storage."** False. Snowflake Unistore has been generally available since November 2024, TiDB X moved persistence to object storage in October 2025, and Databricks announced LTAP in June 2026.
2. **"Kaveon is a transactional database today."** False. The SQL layer parses a bounded DML surface and returns an AST; no row execution, parameter binding, constraint checking, isolation, or durable commit exists. Production transaction startup deliberately returns HTTP 503 because no ADLS product store is configured.

What remains defensible is narrower, accurate, and still differentiating: Kaveon is the only open-source, self-hostable platform pairing a unified transactional and analytical engine over customer-owned storage with a deterministic, model-free natural-language interface. The second half of that sentence ships today. The first half is a target with published acceptance gates.

## Market map

| Platform | Engines | Durable authority | One copy for writes | Open source | Status |
|---|---|---|---|---|---|
| Snowflake Unistore | One platform, Hybrid Tables | Proprietary format on object storage | Yes | No | GA November 2024 on AWS |
| Databricks LTAP | **Two**: Postgres (Lakebase) and Spark/Photon | Object storage, Delta and Iceberg | Claimed, no sync pipelines | Partial | Announced June 2026, "coming soon" |
| TiDB X | One | **Object storage as single source of truth** | Yes | **Apache 2.0** | Announced October 2025; GA unconfirmed |
| Microsoft Fabric SQL database | SQL plus Spark | OneLake | **No** — near-real-time mirror to a read-only Delta copy | No | GA |
| **Kaveon** | One (target) | ADLS Gen2 (target) | Target | **MIT** | Analytics alpha; transactional layer not implemented |

## Platform notes

### Snowflake Unistore

Hybrid Tables reached general availability in November 2024 on AWS, adding row-oriented storage with high-concurrency point operations alongside analytical tables in the same database. Snowflake reports double-digit millisecond point operations executing beside analytical queries. This is the closest existing counterexample to any Kaveon claim of novelty in unifying the two workloads. It is proprietary and SaaS-only.

### Databricks LTAP

Databricks announced Lake Transactional/Analytical Processing in June 2026, describing OLAP and OLTP over a single copy of data in open object storage under one governance model, explicitly eliminating synchronization pipelines. Two qualifications matter. LTAP composes **two independent engines** — Postgres via Lakebase for transactions and Spark/Photon for analytics — rather than one engine serving both. And Databricks lists availability as "coming soon as part of Lakebase", so it is a roadmap position rather than shipped capability.

### TiDB X

TiDB is an open-source distributed SQL database from PingCAP, MySQL wire-compatible, Apache 2.0 licensed, with TiKV as a CNCF Graduated storage layer. Its HTAP model pairs row-oriented TiKV with TiFlash, a columnar engine that replicates from TiKV in real time and serves analytical queries without disturbing transactional performance; a unified SQL layer routes each query to the appropriate engine. TiDB X, announced October 2025, re-architects persistence so that object storage becomes the single source of truth and TiKV becomes stateless with snapshot storage in S3.

**TiDB X is Kaveon's closest architectural neighbor and the strongest counterexample to an open-source differentiation claim.** The material difference is directional. TiDB is a transactional database that grew analytical replicas, so TiFlash analyzes TiDB's own rows. It does not read Parquet, Delta, or Iceberg tables already present in a customer's lake. Adopting TiDB for existing lake data requires migration; adopting Kaveon requires catalog registration. Approximately one third of production TiDB clusters deploy TiFlash at all, indicating most deployments use it as a scale-out transactional database rather than as HTAP.

### Microsoft Fabric SQL database

Creating a SQL database in Fabric replicates its data in near real time into OneLake as a **read-only** Delta copy reachable through the SQL analytics endpoint. Transactional writes and analytical reads therefore address different artifacts joined by managed replication. Fabric's "one copy" describes governance and storage location rather than a single writable representation, which is a meaningful distinction when comparing against architectures whose commit point and analytical scan target the same objects.

## Where Kaveon differs

Two differences survive scrutiny.

**Data is queried in place.** Kaveon reads customer Parquet and Delta files where they already reside and registers them through the catalog. Snowflake and TiDB require data to be loaded into engine-owned storage before either can serve it. Fabric mirrors into OneLake. For an organization whose data already sits in ADLS Gen2, this is the difference between a registration and a migration.

**Natural language is deterministic.** The Kaveon DLM resolves questions to SQL and to precomputed context without a hosted language model, so the same question yields the same answer and the derivation is auditable. Competing platforms reach for a generative model at this layer, which cannot offer reproducibility. No platform surveyed here provides a deterministic natural-language interface, and TiDB provides no semantic or natural-language layer at all.

## Maturity, stated honestly

Kaveon is behind every platform in this document on transactional maturity, and materially so. TiDB has approximately a decade of production operation and a CNCF Graduated storage engine. Snowflake Unistore has been generally available for close to two years.

Kaveon's current transactional position is a foundation: typed product records with revision-checked create, update, and delete; uniqueness and reference validation; immutable digest-verified documents published before a conditional head update; bounded snapshot reads and cursor pagination. Catalog validation is 36 tests with strict Clippy clean. The gaps are enumerated in [the native engine target](../engineering/native-engine-target.md) and [the ADLS transaction protocol](../engineering/adls-transaction-protocol.md); the largest are row DML execution, constraints and isolation, index shard splitting beyond the current 1,024-operation bound, verified head recovery against a live account, and the entire migration, fencing, and rollback path. PostgreSQL remains the authoritative product store.

Analytical maturity is separately assessed at 74/100 in [the readiness qualification](../engineering/engine-readiness-qualification.md), with the 1.9x throughput objective unmet at a measured 1.057x.

## The claim Kaveon may make

> The only open-source, self-hostable platform that unifies transactional and analytical execution over storage the customer owns, with a deterministic, model-free natural-language interface above it.

Present the transactional half as a target with published gates. A visible rubric that Kaveon currently fails is a stronger credibility position than a superlative that a competitor's product page disproves during questions.

## Proof required before any comparative claim

- Transactional correctness against PostgreSQL and analytical throughput against Trino as separate suites, with matched data, resources, concurrency, semantics, and cache state.
- Result hashes, throughput, p50/p95/p99, conflicts and errors, bytes and rows scanned, memory, network, storage requests, and cost per workload.
- Repeated measurements with declared warmups and sample counts, publishing failures and unsupported cases alongside successes.
- The machine-readable gate at `engine/qualification/comparison_gate.py` must report qualified. Missing inputs report `pending` and failed gates report `not_qualified`; neither state may be published as a win.

## Primary references

- [Snowflake: Hybrid Tables now generally available](https://www.snowflake.com/en/blog/unistore-general-availability/)
- [Snowflake: Unistore general availability press release](https://www.snowflake.com/en/news/press-releases/snowflakes-unistore-unifies-transactional-and-analytical-data-with-the-general-availability-of-hybrid-tables/)
- [Databricks: LTAP launch announcement](https://www.databricks.com/company/newsroom/press-releases/databricks-launches-ltap-first-lake-transactionalanalytical)
- [Databricks: what is a Lakebase](https://www.databricks.com/blog/what-is-a-lakebase)
- [TiDB X architecture](https://docs.pingcap.com/tidbcloud/tidb-x-architecture/)
- [TiDB HTAP overview](https://docs.pingcap.com/tidb/stable/explore-htap/)
- [TiFlash overview](https://docs.pingcap.com/tidb/stable/tiflash-overview/)
- [Microsoft Fabric SQL database overview](https://learn.microsoft.com/en-us/fabric/database/sql/overview)
- [Microsoft OneLake overview](https://learn.microsoft.com/en-us/fabric/onelake/onelake-overview)
- Kaveon implementation truth: `HANDSHAKE.md`, `docs/engineering/native-engine-target.md`, `docs/engineering/adls-transaction-protocol.md`, `engine/DISTRIBUTED_EXECUTION_STATUS.md`
