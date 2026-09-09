# Product catalog migration to ADLS

Kaveon will move durable product and Engine-definition state to an ADLS-backed
catalog. PostgreSQL remains the production authority until the protocol,
backfill, reconciliation, fencing, and rollback gates in this document have
passed. This is a design, not an implemented migration and not evidence that a
read-only Delta snapshot can accept product CRUD.

The durable state model is defined in
[ADLS transaction protocol](adls-transaction-protocol.md). It uses immutable
Parquet/data and manifest objects with one conditional catalog head update as
the commit point. It does not use SQLite, a PVC, local files, blob listings, or
an uncoordinated Delta log as durable transaction authority. Process memory and
short-lived upload staging are permitted only before the commit point; a restart
may discard them without changing committed state.

## Inventory and boundaries

The checked-in PostgreSQL schema is
[`api/schema_postgresql.sql`](../../api/schema_postgresql.sql). The migration
inventory is:

| Family | Tables | Important behavior |
| --- | --- | --- |
| Semantic objects | `datasets`, `dataset_dimensions`, `dataset_columns`, `dataset_metrics`, `charts`, `dashboards`, `favorites` | Cascading dataset children, chart/dataset links, JSON payloads, visibility and ownership |
| User state and audit | `saved_queries`, `query_history`, `activity`, `user_themes`, `user_recents` | Per-user access, ordered/paginated reads, unique recent items and audit ordering |
| Engine control plane | `catalog_sources` and Engine catalog/schema/table definitions | Lifecycle validation, unique names, revisions, audit, source-to-definition mapping |
| Adaptive/DLM state | `context_snapshots`, `context_answer_cache`, `dlm_artifact`, `dlm_value_index`, `dlm_router` | Unique keys, bounded cache semantics, generated artifacts and value indexes |
| Legacy chat | `chat_sessions`, `chat_messages` | Owner checks, ordered messages, potentially sensitive payloads; separately provisioned today |

There is no canonical product users table. API authorization derives a verified
Entra principal and role; email is presently used in many ownership rows. The
target object model must retain immutable tenant and provider-subject identity
when available, with email only as a mutable display/lookup attribute.

`data_sources.connection_string`, AI/API/Engine credentials, keyrings, Auth.js
session material, and environment configuration are not product catalog
objects. They remain in their key-managed secret boundary. Query text, chat
messages, cached answers, and DLM values are customer data; retention,
encryption scope, deletion, and access policy must be decided before copying
them.

## Why the current implementation cannot cut over

The Engine catalog currently persists catalog/schema/table definitions in local
SQLite/WAL and exposes revisions for each definition. It has neither an ADLS
writer nor a generic product-object API, and its transaction scope is one
definition operation. The API metadata adapter exposes independent
`query`/`execute` calls; its PostgreSQL connections use autocommit. Existing
multi-write operations consequently cannot be reproduced by replacing one SQL
query with a Parquet write.

The target must preserve primary/unique/foreign-key-like rules, ownership and
role checks, pagination and ordering, and atomic cross-table changes. SQL
dialect translation (`TOP`, `MERGE`, `OUTPUT`, JSON expressions and PostgreSQL
upserts) is a source-adapter detail, not a target contract.

## Required target contract

Each product mutation is a typed transaction request with authenticated tenant,
immutable actor subject, effective role, request/trace IDs, a client idempotency
key plus canonical request digest, optional expected catalog revision, and a
bounded mutation set over declared table schemas.

The coordinator validates authorization, schema versions, primary/unique keys,
foreign references, lifecycle rules, and mutation bounds against one pinned
catalog head. It writes immutable replacement data/index objects and a complete
candidate manifest, then conditionally updates the one catalog head. A failed
head compare-and-swap changes no visible state. The successful head is the only
transaction authority; readers pin it before resolving any table.

The first deliverable is the ADLS catalog writer, reader, head CAS path,
manifest validator, and a bounded dashboard/chart/favorite repository family.
That family proves cross-object validation, visibility, owner-scoped favorite
changes, idempotency, rollback, and read parity before dataset/DLM/history/chat
families move.

## Backfill, dual write, and cutover

1. **Prepare.** Version every target table schema and make all PostgreSQL writes
   for the selected family pass through a transaction wrapper. Add a source
   outbox row in that same PostgreSQL transaction.
2. **Snapshot.** Take a repeatable PostgreSQL snapshot with a recorded source
   watermark. Export typed, redacted records in dependency order to immutable
   ADLS objects; import through the normal validator, never a bypass.
3. **Catch up.** Apply outbox events in source sequence order through the ADLS
   transaction API using durable idempotency keys. Reconcile counts, IDs,
   revisions, ownership, references, and canonical payload hashes.
4. **Shadow read.** Compare authorization-filtered PostgreSQL and ADLS reads.
   Do not expose a target result if comparison fails.
5. **Fence and cut over.** Reject new source writes for that family, drain the
   outbox, reconcile the final watermark, then publish the read switch. Retain
   PostgreSQL and its outbox for the rollback window.
6. **Rollback.** Fence target writes, return reads to PostgreSQL, and preserve
   the failed ADLS head and telemetry. PostgreSQL is retired only after a later,
   separately approved retirement gate.

During transition PostgreSQL is source-authoritative with a durable outbox;
there is no claim of cross-system atomic commit. After cutover the ADLS service
is the family’s sole writer. A reverse mirror needs its own protocol and proof.

## Acceptance and telemetry gates

Before any family cutover, prove rejected mutations leave every table at the
previous pinned head; concurrent writers produce one winner and one retryable
revision conflict; ambiguous responses resolve through the idempotency key;
restart cannot expose a partial transaction; constraints do not require an
unbounded scan; authorization matches today’s API; and final fenced backfill
reconciles IDs, revisions, ownership, references, visibility-filtered reads and
payload hashes.

Emit structured metrics and immutable audit records for commit latency, head
revision, CAS conflicts/retries, validation failures, idempotency replays,
orphan bytes/age, garbage collection, source/target watermark lag, mismatches,
fencing duration, and rollback state. Use IDs and classified error codes rather
than query text, chat content, credentials, or cached result payloads.
