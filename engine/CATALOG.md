# Kaveon Native Catalog

## Purpose

The Kaveon catalog is the Engine's metadata authority. It maps stable catalog, schema, and table identities to storage locations and table metadata without making Hive Metastore a mandatory dependency. Query execution remains direct: Kaveon resolves metadata, then reads Parquet files or replays Delta transaction logs at the registered location.

The current implementation is a durable single-coordinator foundation. It is not yet a distributed metadata service.

## Shipped architecture

```text
Engine catalog API
        │
        ▼
Native catalog store ── SQLite transaction + WAL durability
        │
        ├── immutable definitions, stable IDs, revisions, lifecycle, audit events
        │
        ▼
CatalogManager snapshot ── process-local read path used by planning
        │
        ▼
Self-contained executable fragments
        │
        ▼
Direct Parquet / Delta access ── local storage today; ADLS Gen2 next
```

SQLite is the durable metadata system of record for one coordinator. WAL mode provides crash-safe commits and practical concurrency between catalog readers and the single writer. After a committed mutation, the coordinator refreshes the runtime `CatalogManager` snapshot used by existing SQL resolution and planning. The snapshot is derived state: restarting the coordinator reconstructs it from the durable catalog.

Workers do not consult SQLite or depend on a shared in-memory catalog. Executable fragments carry the resolved table format, storage location, projected columns, predicates, and split assignment required for execution. This keeps task retries deterministic and avoids a metadata lookup on every worker task. A fragment therefore represents the coordinator's catalog revision at planning time; future long-running-query validation can reject incompatible metadata changes explicitly.

## Metadata model

Catalog, schema, and table definitions are immutable values with stable opaque identifiers and monotonically increasing, nonzero revisions. Updates produce a new revision rather than mutating an observed definition in place. Names are user-facing lookup keys; IDs are the durable identity used by persistence and relationships.

Lifecycle transitions are validated. Definitions move through `Draft`, `Active`, `Suspended`, `Deleting`, and `Deleted` only along supported transitions. Terminal deletion cannot be reversed accidentally. Mutations are transactional and produce audit records so the actor, operation, object identity, and resulting revision can be reconstructed independently of query history.

Table definitions retain their physical format, relative location, access pattern, and logical columns. The catalog stores metadata, not customer data. Parquet metadata and Delta `_delta_log` state remain at the registered storage location and are interpreted by the storage layer at query time.

## Security boundary

Catalog records may contain a `CredentialReference`, never a password, access key, client secret, token, or connection-string secret. A reference identifies a managed identity, workload identity, environment binding, or external secret-store entry. Credential resolution belongs at the deployment/security boundary and should return short-lived credentials to the storage adapter without persisting them in catalog definitions, audit events, plans, logs, or query history.

The same rule applies to external catalog configuration: endpoints and non-secret identifiers are metadata; authentication material is not.

## External catalog interoperability

The shared contract identifies these adapter families:

- Native Kaveon catalog
- Hive Metastore
- AWS Glue
- Unity Catalog
- Iceberg REST

Capability declarations make discovery, metadata reads, namespace/table lifecycle, statistics, and atomic-commit support explicit per adapter. These are interface contracts only. Hive, Glue, Unity Catalog, and Iceberg REST adapters are not implemented or shipped, and their presence in the type system must not be presented as connectivity support.

Kaveon's native catalog remains the default. Optional adapters are an interoperability layer for estates that already use another metadata authority; they do not sit in the Parquet/Delta data path and do not turn Hive into an Engine dependency.

## Control-plane integration boundary

The platform API currently has its own data-source registry. That registry is not automatically authoritative for Engine planning. A deliberate integration bridge is required to translate Entra-authorized platform operations into Engine catalog API mutations, map platform source identities to stable Engine catalog IDs, pass only credential references, surface revision conflicts, and preserve audit attribution.

Until that bridge is implemented and tested, registering a source in the platform API must not be assumed to register it in the Engine catalog. Studio administration, platform authorization, and Engine metadata resolution are separate control-plane responsibilities with an outstanding synchronization contract.

## Scale path

SQLite/WAL is appropriate for the shipped single-coordinator deployment and local validation. It is not the final design for active-active coordinators: a process-local `CatalogManager` snapshot cannot by itself provide cross-coordinator ordering, invalidation, or conflict detection.

The multi-coordinator target is an external transactional catalog service, initially backed by PostgreSQL. That service must provide:

- atomic compare-and-swap updates by object revision;
- globally consistent ID allocation and uniqueness constraints;
- ordered change publication for snapshot invalidation;
- transactional lifecycle and audit writes;
- coordinator-independent credential references;
- bounded metadata caching with revision-aware invalidation; and
- migration tooling that preserves IDs, revisions, and audit history from SQLite.

The runtime-facing catalog contract should remain stable across that transition. Replacing the persistence and invalidation mechanism must not require storage readers or execution workers to understand PostgreSQL, Hive, or platform API internals.

## Deliberate non-claims

The current catalog does not provide multi-coordinator consistency, external catalog connectivity, distributed locks, automatic synchronization with the platform source registry, Delta checkpoint cataloging, or Iceberg catalog execution. Those are explicit integration and scale gates, not implicit behavior of the shipped single-coordinator catalog.
