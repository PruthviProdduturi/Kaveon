# Product catalog migration assessment

This is a source assessment for moving mutable Kaveon product metadata from the
current PostgreSQL metadata database to an internal catalog. It is not a
completed migration or implementation. No production data was copied in this assessment. In
particular, a readable Delta snapshot is not a mutable metadata database.

## Verified current state

The Engine can read local and object-store Delta tables. `DeltaTableReader` and
`ObjectDeltaReader` replay a Delta v1 snapshot to select active Parquet files;
the server uses `ObjectDeltaReader` for remote Delta scan sources. See
[`delta_reader.rs`](../../engine/crates/storage/src/delta_reader.rs),
[`object_delta.rs`](../../engine/crates/storage/src/object_delta.rs), and
[`delta_snapshot.rs`](../../engine/crates/storage/src/delta_snapshot.rs).

That reader deliberately rejects unsupported Delta features: reader protocol
other than v1, column mapping, deletion vectors, partition reconstruction, Delta
v2 checkpoint sidecars, incomplete JSON history, and oversized metadata. The
Engine has no Delta writer, transaction-log commit coordinator, optimistic
concurrency protocol, table-level authorization, or multi-table transaction.
The only `_delta_log` writes in the repository are test fixtures. An ADLS Delta
snapshot therefore supports analytical reads; it cannot presently replace
PostgreSQL CRUD.

The Engine's own durable catalog is SQLite/WAL with immediate transactions,
optimistic revisions, lifecycle validation, and audit events in
[`engine/crates/catalog/src/lib.rs`](../../engine/crates/catalog/src/lib.rs).
It stores Engine catalog/schema/table definitions, not dashboards, user state,
or the application relational model.

The API talks directly to its metadata database through
[`api/database/metadata.py`](../../api/database/metadata.py) and
[`api/database/pool.py`](../../api/database/pool.py). The pool uses normal SQL
reads and writes and presently enables PostgreSQL autocommit. Product services
depend on SQL filtering, joins, unique constraints, foreign keys, ordered reads,
and in some cases several writes for one user action.

## Source inventory

The additive PostgreSQL schema is
[`api/schema_postgresql.sql`](../../api/schema_postgresql.sql). Its product data
falls into these groups:

| Group | Current tables | Primary API code |
| --- | --- | --- |
| Semantic objects | `datasets`, `dataset_dimensions`, `dataset_columns`, `dataset_metrics`, `charts`, `dashboards`, `favorites` | `api/services/datasets.py`, `charts.py`, `dashboards.py`, `favorites.py` |
| User state and audit | `saved_queries`, `query_history`, `activity`, `user_themes`, `user_recents` | `saved_queries.py`, `query_history.py`, `theme.py`, `user_recents.py` |
| Engine integration | `catalog_sources` | `api/routers/catalog_sources.py`, `api/services/engine_bridge.py` |
| Adaptive/DLM state | `context_snapshots`, `context_answer_cache`, `dlm_artifact`, `dlm_value_index`, `dlm_router` | `api/routers/context.py`, `api/routers/dlm.py` |
| Legacy or separately provisioned chat state | `chat_sessions`, `chat_messages` are used by `api/routers/chat_history.py` but are not created by the checked-in additive PostgreSQL schema | `api/routers/chat_history.py` |

There is no canonical application `users` table in the checked-in schema.
Identity and roles come from verified Entra claims in
[`api/services/users.py`](../../api/services/users.py); user email appears in
ownership and preference rows. A migration must preserve immutable subject/tenant
identity rather than treating a display name or email as the identity key.

Do not copy credentials or configuration into a catalog export. In particular,
exclude `data_sources.connection_string` even when it is an encrypted envelope,
all metadata/Engine/auth environment secrets, credential keyrings, and Auth.js
session secrets. `api/services/credentials.py` shows that connection credentials
have a distinct key-managed lifecycle. Query text, chat messages, cached answers,
and DLM values can contain customer data; inventory and classify them separately
before any export rather than assuming they are harmless metadata.

## Concrete blockers

1. **No write authority.** The Engine exposes no API or storage implementation
   that creates Parquet data files and safely commits Delta log versions on ADLS.
2. **No concurrent commit semantics.** Delta migration would need conditional
   object creation, conflict retry, idempotency keys, commit ownership, orphan
   cleanup, and recovery. The reader's snapshot replay does not supply any of
   these writer guarantees.
3. **No relational integrity or cross-object atomicity.** Dataset children,
   dashboard/chart references, favorites, ownership changes, and chat/session
   updates need foreign-key-like checks and atomic multi-row transitions. One
   Delta table commit cannot atomically update several tables.
4. **API is SQL-shaped.** Existing services embed SQL predicates, ordering,
   joins, generated IDs, and dialect adaptation. A Delta-backed repository layer
   must replace these calls and preserve authorization, pagination, visibility,
   and compare-and-swap behavior.
5. **Identity, privacy, and secrets need a boundary.** Entra subjects, user-owned
   data, query/chat contents, and encrypted connection references require data
   classification, retention, deletion, and key-management decisions before a
   bulk copy.
6. **No migration safety mechanism exists.** There is no backfill tool,
   change-capture/dual-write protocol, reconciliation suite, read cutover, or
   rollback path for this schema.

## Feasible milestones

The requested target is the `Kaveon` internal product catalog, with all current
PostgreSQL system-table behavior preserved. The migration must not be declared
complete on the basis of an export, a read-only snapshot, or a subset of CRUD.
Required acceptance includes commit/rollback across related records, concurrent
update conflict tests, primary/unique/referential constraints, durable recovery
after interrupted writes and coordinator restart, existing role/ownership checks,
and a validated backfill/cutover/rollback for every system-table family. PostgreSQL
stays authoritative until those gates pass. This is the next engineering task;
the OpenSource analytics import does not implement it.

1. Define the supported scope and a versioned product-object model. Keep
   credentials/auth configuration outside it; decide whether history, chat, and
   DLM caches are migrated, archived, or retained in PostgreSQL.
2. Build a dedicated mutable catalog service before changing API storage. For a
   Delta design, implement an ADLS writer and commit coordinator with conditional
   commits, revisions/idempotency, recovery, audit, and per-object authorization.
   Alternatively retain a transactional control-plane database and use Delta for
   append-only analytical projections.
3. Add an API repository interface and migrate one bounded object family first
   (for example dashboards plus charts and favorites). Preserve ownership,
   visibility, ordering, and referential checks; add concurrent update and
   restart/recovery tests.
4. Create a read-only, redacted inventory exporter and a deterministic backfill
   validator. Reconcile row counts, IDs, revisions, references, ownership, and
   visibility before enabling dual reads.
5. Introduce dual writes with an outbox or equivalent durable change stream,
   then compare reads continuously. Cut over one family only after reconciliation
   and rollback are demonstrated. Do not present a Delta snapshot as evidence of
   a completed mutable-metadata migration.
