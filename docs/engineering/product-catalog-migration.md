# Product catalog migration to ADLS

Kaveon will move durable product and Engine-definition state to an ADLS-backed
catalog. PostgreSQL remains the production authority until the protocol,
backfill, reconciliation, fencing, and rollback gates in this document have
passed. The manifest/CAS transaction substrate, authenticated product CRUD
endpoint, and typed API client now exist. PostgreSQL repository integration,
backfill, outbox, reconciliation, fencing and cutover do not; this document is
therefore not evidence that PostgreSQL can be retired.

The Engine implements immutable documents and manifests with an ADLS head CAS,
bounded typed product records, optimistic revisions, owner-scoped reads and
writes, and application-level head history/recovery. The remaining statements
distinguish those implemented primitives from migration and deployment gates
that still require qualification.

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

The schema file is not the whole live inventory. `dlm_answers` and `dlm_sketch`
are created by `api/dlm/engine.py`; `chat_sessions` and `chat_messages` live in
`data/migrations/chat_history.sql`; and `ai_providers` and `user_ai_keys` are
created at runtime by `api/services/ai_service.py`. A migration inventory must
discover the deployed database as well as compare it with these definitions.

### Service and dependency map

| Order | Objects | Writers and readers | Required invariants |
| ---: | --- | --- | --- |
| 0 | Entra principal and role mapping | authentication middleware | Tenant plus immutable provider subject is authoritative; email is mutable metadata |
| 1 | `catalog_sources`, `data_sources` | catalog-source/data-source routers, Engine bridge, SQL routing | Unique source identity/name, valid lifecycle, encrypted credential references only |
| 2 | `datasets` | dataset service, chat, SQL and Lab routers | Stable ID, owner/visibility, valid physical catalog/schema/table reference |
| 3 | `dataset_dimensions`, `dataset_columns`, `dataset_metrics` | dataset service, query generator, chat, DLM compiler | Existing parent; unique semantic identity; replace-all edits are atomic |
| 4 | `charts` | chart service and dashboard renderer | Existing dataset, typed query/viz configuration, owner/visibility |
| 5 | `dashboards` | dashboard service and DLM curator | Chart and filter-dataset references resolve at one revision; layout and chart list change together |
| 6 | `favorites`, `saved_queries`, `user_themes`, `user_recents` | corresponding services | Owner-scoped uniqueness and authorization-filtered reads |
| 7 | `query_history`, `activity` | query history and catalog-source audit | Append identity, trace/query ID, deterministic ordering and retention |
| 8 | `chat_sessions`, `chat_messages` | chat-history/chat routers | Session owner check; message append and session timestamp update are atomic |
| 9 | `context_snapshots`, `context_answer_cache`, `dlm_artifact`, `dlm_value_index`, `dlm_router`, `dlm_answers`, `dlm_sketch` | DLM profiler/router/engine | Existing dataset and one source revision; a complete generation publishes atomically |

`ai_providers`, `user_ai_keys`, `data_sources.connection_string`, and other
encrypted credential envelopes stay in the key-managed secret boundary.
KaveonDB may store a non-secret reference and rotation metadata, but ordinary
catalog Parquet must not contain the credential envelope.

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

Several compound mutations therefore lack an atomic boundary today: dataset
creation and semantic child inserts; delete-and-recreate semantic edits;
dashboard/chart cleanup and imports; data-source favorite replacement; chat
message append plus session timestamp update; and DLM answer/value/sketch
replacement plus artifact publication. These need explicit repository
transactions before dual write. An outbox added after the writes would still
permit a partial source mutation.

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

Catch-up implementation status: the API has a bounded sequence-ordered replay
library for typed product families. It validates source hashes, uses target
revision compare-and-swap, resolves lost target responses by exact document
comparison, locks before source acknowledgment, and stops at the first failure.
It is not scheduled or deployed, and it has no initial backfill watermark;
live catch-up and reconciliation remain unproven.

Snapshot implementation status: datasets and their semantic children can be
captured under one PostgreSQL repeatable-read transaction with a recorded
outbox watermark, stable ordering, bounded row/byte limits and deterministic
record/snapshot hashes. The backfill path creates only missing target records
and exact-reads every target before emitting its report. It is an unscheduled,
unexecuted library; a real snapshot artifact, concurrent source-write test and
post-catch-up reconciliation report are still required.

## Staged KaveonDB delivery

KaveonDB is the logical transactional product database. Its durable tables and
indexes may use the ADLS manifest protocol, but callers use typed repositories
and transactions instead of arbitrary metadata SQL.

### Stage 0 — contract and PostgreSQL transaction repair

Status: **dataset source conversion implemented; live qualification pending.** PostgreSQL
has a connection-pinned unit-of-work API and a bounded, sequenced, canonical
product migration outbox with event-ID/digest replay validation. Dataset parent
and semantic-child writes use it atomically in source, with failure injection at
each statement. The schema is not deployed and the full repository set has not
been converted, so the Stage 0 exit gate has not passed.

Define typed schemas, canonical JSON/timestamp encodings, immutable
tenant/subject identity, foreign and unique constraints, revision tokens,
idempotency keys and retention classes. Add a PostgreSQL unit-of-work API and
move every compound mutation into it. Add one outbox record in that same commit.

Exit when failure injection after every statement proves all-or-nothing source
state and exactly one durable outbox event per committed request.

### Stage 1 — KaveonDB transaction substrate

Implement versioned schemas, immutable data/index objects, manifest validation,
one conditional head update, pinned-head readers, request-digest idempotency,
bounded constraint indexes and application-level head history. Prove concurrent
conflicts, lost-response replay, restart recovery, corruption detection and
restoration from a verified prior head.

### Stage 2 — bounded product objects

Move `datasets` plus semantic children, then `charts`, `dashboards` and
`favorites`. This slice exercises parent/child cascades, JSON payloads,
cross-object references, visibility, owner uniqueness and dashboard changes
spanning multiple objects. Backfill in dependency order, replay the source
outbox, and run authorization-filtered shadow reads. PostgreSQL stays
authoritative.

### Stage 3 — sources and personal state

Move non-secret `catalog_sources` and `data_sources` metadata, saved queries,
themes and recents. Keep secret values in their existing key-managed boundary.
Resolve a source and its Engine definition at one pinned KaveonDB revision.

### Stage 4 — append-heavy history and chat

Move activity, query history and chat only after partitioning, ordered cursor
reads, retention/deletion and payload-encryption policies exist. High-volume
history must not rewrite unrelated product objects.

### Stage 5 — derived context

Rebuild DLM/context state from cut-over dataset definitions and current data
instead of treating old derived rows as authority. Publish each dataset's
artifact, router, values, answers and sketches against one source revision. An
old generation cannot remain routable after the dataset revision changes.

### Stage 6 — family-by-family cutover

Fence one repository family’s PostgreSQL writers, drain through a recorded
outbox sequence, reconcile the final watermark, switch reads, then enable
KaveonDB writes. Do not run unfenced bidirectional writes. Retain PostgreSQL and
its outbox throughout the rollback window.

## Dual-write and rollback criteria

Dual write means a PostgreSQL transaction plus outbox followed asynchronously
by idempotent KaveonDB apply. It is not two best-effort synchronous writes. The
target apply key is `(tenant, repository family, source sequence)`; reusing it
with another request digest is rejected.

A family may cut over only when outbox lag is zero at the fence; counts, IDs,
canonical payload hashes, revisions and references reconcile at one watermark;
role-filtered list/detail reads match for Admin, Analyst and Viewer; CRUD,
cascade, replay and revision-conflict tests pass; pod restart and ambiguous
timeout tests never expose a partial revision; and dashboard/chart resolution
pins one catalog revision.

Rollback is mandatory for a reconciliation mismatch, invariant bypass, lag
breach, commit-error-budget breach, invalid head, or authorization difference.
Fence KaveonDB writes, preserve the failing head and telemetry, switch reads to
PostgreSQL, and do not replay target-only writes without a separately tested
reverse-reconciliation procedure.

## AKS acceptance suite

Run against an isolated namespace/storage prefix with three Engine workers and
at least two API replicas:

1. Seed sources, a dataset with semantic children, charts, a dashboard,
   favorites, saved queries, history and chat; record hashes and head revision.
2. Execute the API CRUD/role matrix. Reject cross-tenant and another user’s
   private-object access.
3. Update a dataset and children while rendering its dashboard. Each request
   sees the prior or new revision, never mixed references.
4. Race two updates with one expected revision. Require one commit and one
   retryable conflict; replay the winner’s idempotency key without a new head.
5. Kill coordinator/API pods before upload, after upload and around head CAS.
   The visible head remains complete; orphan data is unreachable and collected.
6. Inject ADLS timeout/throttling and ambiguous responses. Bounded retries must
   produce no duplicate object, favorite, message or outbox application.
7. Corrupt an isolated candidate manifest/index. Readers reject it and recover
   only from verified head history, never blob-listing order.
8. Backfill while source writes continue, catch up, fence, reconcile, shadow
   read and cut over. Render the canonical eight dashboards and execute all 70
   chart definitions at the final watermark.
9. Force every rollback trigger and meet the recovery objective without object
   or authorization loss.
10. Rebuild DLM after object cutover. Verify no stale dataset IDs, source-revision
    agreement, exact eligible hits and SQL fallback for stale, approximate or
    unsupported context.

Archive image digests, watermarks, KaveonDB head, hash reconciliation, role and
failure matrices, dashboard query IDs, latency percentiles and rollback times.

## Acceptance and telemetry gates

Before any family cutover, prove rejected mutations leave every table at the
previous pinned head; concurrent writers produce one winner and one retryable
revision conflict; ambiguous responses resolve through the idempotency key;
restart cannot expose a partial transaction; constraints do not require an
unbounded scan; an unavailable/corrupt head is restored only from verified
application-level backup evidence rather than a blob listing; authorization
matches today’s API; and final fenced backfill reconciles IDs, revisions,
ownership, references, visibility-filtered reads and payload hashes.

Emit structured metrics and immutable audit records for commit latency, head
revision, CAS conflicts/retries, validation failures, idempotency replays,
orphan bytes/age, garbage collection, source/target watermark lag, mismatches,
fencing duration, and rollback state. Use IDs and classified error codes rather
than query text, chat content, credentials, or cached result payloads.
