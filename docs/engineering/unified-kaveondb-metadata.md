# Unified KaveonDB metadata architecture

Status: **accepted product direction; implementation and cloud qualification pending**

## Decision

KaveonDB is the durable transactional authority for both application product
state and editable Engine catalog definitions. There is one logical commit
history per KaveonDB deployment. Studio, API, SQL Lab and Engine coordinators
read and write through KaveonDB's authenticated transaction/catalog APIs; no
application data or editable catalog definition uses PostgreSQL as a fallback.

The SQL namespace is a projection of that authority, not another copy:

```text
KaveonDB (one transactional authority; versioned snapshots and audit)
  product    datasets, dataset semantics, charts, dashboards, sources, DLM,
             saved queries, user preferences, activity, chat and query history
  catalog    catalog, schema and table definitions, lifecycle and revisions
  system     read-only operational metadata and administrator diagnostics

OpenSource  public analytical datasets
Kaveon      Kaveon-owned analytical/product-usage datasets
```

Product and catalog records are read-only through SQL. Mutations use typed,
authorized APIs and transactions so validation, ownership, CAS revision checks,
audit and cross-record constraints cannot be bypassed with SQL. The `system`
catalog is Admin-only. Standard `information_schema` is filtered to objects the
caller is allowed to see; it is not an Admin-only dump of hidden objects.

## What belongs where

Every PostgreSQL application table is inventoried and mapped into a typed
KaveonDB product family or a related child object; it is not copied into an
analytical catalog as a raw table. The currently configured 16 authority
families are:

| KaveonDB namespace | Migrated families |
|---|---|
| `product` | `datasets`, `dataset_semantics`, `charts`, `dashboards`, `sources`, `saved_queries`, `favorites`, `user_themes`, `user_recents`, `activity`, `chat_history` |
| `product.dlm` | `dlm_generation` (typed definitions, runs, immutable compiled artifacts) |
| `product.observability` | `query_history` |
| `product.configuration` | `ai_configuration`, `context_cache` |
| `catalog` | Engine catalog/schema/table definitions, revisions, lifecycle, audit and ownership; migrated from the Engine SQLite catalog, not a PostgreSQL family |

`dataset_semantics` includes columns, dimensions, measures and filters formerly
held in normalized child tables. Identity-provider credentials, OAuth tokens,
and session secrets are not migrated as product records. A complete retirement
claim still requires a live source-table inventory against this family map;
the family manifest alone does not prove every legacy PostgreSQL table was
covered.

## Product and access model

- Admins can inspect read-only `KaveonDB.product` and `KaveonDB.catalog` views
  in SQL Lab, subject to sensitive-field redaction. Writes remain on the
  transactional API.
- `KaveonDB.system` exposes operational diagnostics to Admins only.
- User-owned records remain owner-scoped in APIs and filtered SQL views.
- Catalog ACL grants are a separate typed `catalog_access` family in the same
  KaveonDB transaction authority (revision-CAS and audit); they are not copied
  into PostgreSQL or SQLite. Existing catalog/schema/table definitions remain
  SQLite-backed until the separately gated catalog-store migration passes.
- `OpenSource` contains public-source datasets only. `Kaveon` contains
  Kaveon-owned analytical data, including synthetic usage/telemetry tables.
- Qualification fixtures live in a separate non-product qualification catalog
  or test environment; they do not appear in the user-facing `Kaveon` catalog.

## Deployment contract

The logical schema and commit protocol are cloud-neutral; each installation
selects exactly one durable backend:

| Mode | Durable backend | Identity |
|---|---|---|
| Local Docker | local filesystem using the same immutable-object and conditional-head protocol | local installation identity; no cloud credential required |
| AKS | ADLS Gen2 conditional object store | AKS workload identity; no storage key in catalog documents |
| AWS | S3 conditional object store | workload IAM role; no access key in catalog documents |

The existing product transaction implementation supports local filesystem and
ADLS Gen2. **S3 is not yet implemented or qualified**, and the Engine catalog
definitions still use coordinator-local SQLite/WAL. Therefore the cross-cloud
contract is a target, not current portability evidence. A backend is supported
only after conditional-create/CAS semantics, authorization, ambiguous-write
recovery, restart, concurrency and restore tests pass against that backend.

## Migration and rollout gates

1. Add catalog/schema/table definitions and their audit/revision records as
   typed KaveonDB transactional records. Keep the current SQLite store as a
   temporary read-only migration source/cache; do not dual-author it.
2. Implement a coordinator snapshot projection from one pinned KaveonDB
   generation. Distribute the exact generation to workers and fail closed when
   a worker is behind. Cache state is disposable and rebuildable.
3. Migrate SQLite catalog definitions/statistics and every PostgreSQL table
   family with per-family counts, canonical hashes, reference checks and
   owner/visibility parity. Recompute derived statistics only where doing so
   preserves qualified semantics.
4. Expose read-only, permission-filtered SQL views; test Admin, Editor, Analyst
   and Viewer visibility and prove SQL DML cannot mutate protected metadata.
5. Run restart, competing-writer, lost-response, backup/restore and rollback
   tests on local, ADLS/AKS, and S3/AWS independently.
6. Remove SQLite as authority only after a full reconciliation and durable
   worker-snapshot rollout. Keep it only as a disposable cache if useful.

## Current state

- Product records use KaveonDB's immutable product transaction catalog, stored
  locally under `data/adls-mirror/product-transactions` in this Docker setup.
- Curated analytical namespaces are `OpenSource` and `Kaveon`; the latter
  groups Kaveon-owned showcase tables under `usage`. `KaveonDB` currently names
  the transactional authority in the product/API, not a queryable SQL catalog.
- Catalog/schema/table definitions, statistics and catalog audit still use
  `kaveon_catalog-data` (SQLite/WAL) locally.
- The SQL `KaveonDB` system/product/catalog views described above do not yet
  exist. Product records currently use the authenticated transaction API.
- The catalog manifests and showcase importer now define the logical move to
  `Kaveon.usage`; they preserve the existing Kaveon Events dataset ID and keep
  the same ADLS paths. This branch has not registered the manifests against a
  running Engine, rebound live dataset records, or passed dashboard/DLM
  qualification. Existing coordinator catalog state therefore remains
  unchanged until that rollout is explicitly performed.
- PostgreSQL is not started in the local Compose profile. That proves the local
  runtime can operate without a PostgreSQL service; it does not prove that the
  full historic PostgreSQL table inventory has been reconciled.
