# Kaveon Engine and Microsoft Fabric SQL Analytics Endpoint

> Verified September 4, 2026. This compares product architecture and current capabilities; it is not a price or performance benchmark. Kaveon is a personal independent project and must use resources in its own account/subscription, never an employer-owned environment.

## Executive position

The direct comparison is between Kaveon Engine and the automatically provisioned SQL analytics endpoint of a Microsoft Fabric Lakehouse. Both expose analytical SQL over lake data. The surrounding product boundaries differ: the endpoint belongs to the managed Fabric and OneLake ecosystem, while Kaveon Engine belongs to the independent Kaveon platform alongside DLM and Studio.

Kaveon should not position the Engine as a complete Fabric replacement. Its credible engine-level differentiation is a portable, customer-storage-first execution path whose planner, scheduler, exchanges, operators, and readers are owned end to end. The Fabric endpoint's advantages are managed operations, automatic Lakehouse integration, enterprise identity/governance, and direct participation in the Power BI and OneLake ecosystem.

## Engine and endpoint boundary

| Dimension | Kaveon Engine `dev` | Fabric Lakehouse SQL analytics endpoint |
|---|---|---|
| Delivery | Self-hostable Rust coordinator/workers; hosted Engine pending | Microsoft-managed endpoint created with each Lakehouse |
| Storage center | Registered customer-controlled Parquet/Delta locations | Lakehouse Delta tables in OneLake plus supported shortcuts |
| SQL dialect | Focused analytical SQL surface | Read-oriented T-SQL surface with DQL and limited DDL |
| Write/transaction model | Read path today; optimized ingest target | Endpoint is read-only for table data; Fabric Warehouse is the transactional SQL offering |
| Distributed runtime | Alpha stage/task/split/exchange execution | Managed implementation, not customer-operated |
| BI integration | Kaveon Studio target integration | Power BI semantic models and Direct Lake ecosystem |
| Natural language | DLM sits above Engine as a separate deterministic layer | Copilot belongs to the surrounding Fabric experiences |

## Similar ideas, different contracts

Fabric shortcuts make referenced data appear in the OneLake namespace so Spark, SQL, Real-Time Intelligence, and Analysis Services can access it. Kaveon's Live Lake Path also avoids mandatory import, but it should not copy Fabric terminology or imply identical semantics. Kaveon registers a physical location in its own catalog, resolves a table snapshot, and sends bounded work directly to Engine workers.

Fabric can also mirror operational databases into OneLake as Delta. That is replication. Kaveon's default lake path is direct read; its planned optimized ingest is an explicit optional rewrite into customer-controlled storage. These modes must remain visibly distinct.

## SQL and execution

The Lakehouse SQL analytics endpoint is read-oriented, automatically provisioned, and queries Delta tables and supported shortcuts. It must not be confused with Fabric Warehouse, which provides the broader T-SQL, DML/DDL, and transactional warehouse surface. Kaveon Engine currently implements a smaller analytical SQL surface over local Parquet/Delta, including filters, projection, arithmetic, grouping, exact distinct aggregates, joins, Sort/TopN, and distributed execution foundations.

Kaveon Engine does not currently provide broad T-SQL compatibility, managed Lakehouse discovery, Direct Lake semantic-model behavior, or a managed capacity service. Spark, pipelines, and mirroring are surrounding Fabric workloads rather than capabilities of the SQL analytics endpoint itself. DLM and Studio are likewise surrounding Kaveon components rather than execution operators inside Engine.

## Operational comparison

| Concern | Kaveon Engine `dev` | Fabric SQL analytics endpoint |
|---|---|---|
| Infrastructure operation | User/operator owns deployment | Microsoft-managed SaaS |
| Capacity management | Early worker topology; admission control target | Fabric capacity model and managed workloads |
| Catalog durability | SQLite/WAL for one coordinator | Managed Fabric/OneLake item metadata |
| Multi-coordinator metadata | PostgreSQL-backed service target | Managed by Fabric |
| Observability | Query, stage/task, scan, node-memory telemetry; operator metrics incomplete | Integrated monitoring and capacity experiences |
| Availability/DR | Deployment responsibility; not production-qualified | Service-defined availability and regional capabilities |

## Where Kaveon can differentiate

1. **Deterministic conversational analytics:** DLM answers covered questions through compiled context without requiring a hosted LLM call.
2. **Deployment choice:** the long-term architecture can run in a customer's environment rather than requiring a single SaaS capacity boundary.
3. **Focused native execution:** Kaveon owns the Rust planner, scheduler, exchange, operators, and readers instead of delegating the analytical hot path to an external engine.
4. **Customer-controlled optimization:** optional rewrites remain in storage selected and controlled by the customer.
5. **Unified product contract:** Engine telemetry, DLM evidence, and Studio interaction can share one query identity and explanation model.

These are architectural opportunities, not proof of superiority. ADLS Gen2 execution, integrated identity, catalog synchronization, governance, operational resilience, and comparative benchmarks must land before these become enterprise claims.

## Decision guide

Choose the Fabric SQL analytics endpoint today when the data already belongs in a Fabric Lakehouse and the organization wants managed T-SQL exploration, OneLake shortcuts, Power BI integration, Fabric governance, and capacity-based operations.

Evaluate Kaveon Engine when portability, self-hosting, customer-controlled storage, a focused direct-lake engine, or control of the complete execution path matters more than Fabric's managed integration—and only after validating the required connectors, security, SQL surface, and operations. Deterministic question resolution is a Kaveon platform advantage above the Engine, not an engine benchmark metric.

## Primary references

- [Fabric Lakehouse and Warehouse decision guide](https://learn.microsoft.com/en-us/fabric/fundamentals/decision-guide-lakehouse-warehouse)
- [Fabric data storage options](https://learn.microsoft.com/en-us/fabric/fundamentals/store-data)
- [OneLake shortcuts](https://learn.microsoft.com/en-us/fabric/onelake/onelake-shortcuts)
- [Lakehouse SQL analytics endpoint](https://learn.microsoft.com/en-us/fabric/data-warehouse/get-started-lakehouse-sql-analytics-endpoint)
- Kaveon implementation truth: `ARCHITECTURE.md`, `STATUS.md`, `HANDSHAKE.md`, and `engine/CATALOG.md`
