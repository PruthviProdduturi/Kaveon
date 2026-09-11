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
| Adaptive/DLM state | DLM definitions plus `context_snapshots`, `context_answer_cache`, `dlm_artifact`, `dlm_value_index`, `dlm_router` | Revision-pinned dataset definition; generated runs remain a separate atomic-publication design |
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
| 9 | DLM definition; then `context_snapshots`, `context_answer_cache`, `dlm_artifact`, `dlm_value_index`, `dlm_router`, `dlm_answers`, `dlm_sketch` | DLM profiler/router/engine | Definition is now a typed `dlm_definition` record referencing one dataset revision; generated runs still require atomic generation publication |

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

### Disabled operational backfill command

The command defaults to a read-only source capture and writes an integrity-
checked checkpoint under an operator-selected path:

```powershell
python scripts/backfill-product-catalog.py --checkpoint tmp/dataset-backfill.json
```

The checkpoint contains the exact product documents, owners and SQL metadata
needed to resume the same snapshot. Treat it as customer metadata: keep it in a
restricted directory, never commit it, and retain or delete it under the same
policy as a database export. `tmp/` is ignored by Git. A rerun refuses to
overwrite an existing checkpoint; inspect it with a dry-run resume:

```powershell
python scripts/backfill-product-catalog.py --checkpoint tmp/dataset-backfill.json --resume
```

Target writes require both an explicit flag and enable variable. This is an
operator guard, not cutover authorization:

```powershell
$env:KAVEON_PRODUCT_MIGRATION_ENABLED = "true"
python scripts/backfill-product-catalog.py --checkpoint tmp/dataset-backfill.json --resume --apply
```

After each exactly reconciled record, the command atomically replaces and
flushes the checkpoint with the next position. A crash after target commit but
before checkpoint replacement safely retries that record through exact target
comparison. Completion performs a full-snapshot reconciliation before marking
the checkpoint complete. It does not replay the post-watermark outbox, switch
reads, fence writes, or alter PostgreSQL authority.

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

## Dataset shadow point reads

Set `KAVEON_DATASET_SHADOW_READ_ENABLED=true` to compare authenticated
single-dataset reads. The control defaults off. The comparator removes the
requester's PostgreSQL-only favorite decoration, retains canonical raw semantic
children, and performs one owner-scoped KaveonDB point read using the same actor
and role. Each canonical document is capped at 1 MiB. Logs contain only record
ID, status, byte counts, SHA-256 identities and target generation; errors are
reduced to exception type. PostgreSQL still supplies the unchanged response.
Dataset list reads use the same control and compare only the PostgreSQL list
projection, excluding the caller-specific favorite decoration. At most 25
records are checked through owner-scoped point reads; larger lists emit
`skipped_limit` without target access. List telemetry contains aggregate
match/missing/mismatch counts and order-sensitive batch hashes, never documents
or principals. Internal service reads and every write path remain outside this
slice.

## Dataset post-write observer

Set `KAVEON_DATASET_POST_WRITE_VERIFY_ENABLED=true` to observe committed
PostgreSQL dataset creates, updates and deletes. The control defaults off and
returns before reading the outbox. After the source transaction commits, the
observer reads only that exact outbox event. An event without `applied_at` is
reported as `pending_replay` and causes no target read; replay lag is therefore
not mislabeled as data divergence. An applied create/update is checked against
the owner-scoped KaveonDB record hash, while an applied delete verifies target
absence. Changed or missing source events and target differences have distinct
statuses. Telemetry contains IDs, source sequence, operation, hashes, sizes,
generation, attempt count and bounded error code only. It never contains the
document, actor or owner. Observer failures are reduced to exception type and
cannot change the PostgreSQL mutation response.

## User theme migration

Theme save/update now locks the owner-keyed PostgreSQL row, writes the normalized
lowercase color and appends exactly one canonical `user_themes` outbox event in
the same transaction. Delete locks first and emits one tombstone only when a row
exists. Source or outbox failure propagates so callers cannot observe a false
success, and cache invalidation occurs after the transaction.

The default-dry backfill captures at most 100,000 themes under PostgreSQL
`REPEATABLE READ, READ ONLY` at the outbox watermark. Documents contain exactly
the owner email and validated six-digit color, with deterministic record and
snapshot hashes. `scripts/backfill-user-themes.py` uses an integrity-checked 8
MiB atomic checkpoint; apply requires
`KAVEON_USER_THEME_MIGRATION_ENABLED=true` and advances only after exact
owner-scoped reconciliation.

`KAVEON_USER_THEME_SHADOW_READ_ENABLED=true` enables an owner-scoped KaveonDB
point comparison for cached and uncached reads. Telemetry hashes the record ID
and contains hashes, sizes, generation and status only. Observer failures cannot
change the PostgreSQL response. The source outbox schema is not deployed: a live
read-only AKS probe on September 11, 2026 returned false for
`to_regclass('public.product_migration_outbox')`. No theme migration write can be
enabled until that schema exists and is qualified.

## Chart shadow point reads

Charts are the next typed family after datasets. Set
`KAVEON_CHART_SHADOW_READ_ENABLED=true` to compare authenticated chart point
reads; the control defaults off and returns before target access. The comparison
uses the same requesting actor and role and a fixed canonical projection of
identity, dataset reference, query/viz configuration, visibility, ownership and
timestamps. PostgreSQL-only favorite, joined dataset name, thumbnail preview and
compatibility aliases are excluded. Each side is bounded at 1 MiB. Telemetry
contains only record ID, match/missing/mismatch status, hashes, sizes and target
generation, while PostgreSQL continues to supply the response. Chart lists and
all chart writes remain outside this slice because charts do not yet have the
source outbox/backfill foundation that datasets have.

## Chart snapshot backfill

The chart backfill captures either known modern or legacy charts layout in one
PostgreSQL `REPEATABLE READ, READ ONLY` transaction at the product-outbox
watermark. It parses query and visualization configuration strictly, excludes
favorite, thumbnail and joined dataset-name decorations, and rejects missing
owners, datasets, invalid visibility or malformed configuration. Each document
is limited to 1 MiB and a snapshot to 10,000 charts.

For every chart, capture owner-reads the referenced KaveonDB dataset and requires
all datasets at one target snapshot. The chart document and typed Engine
reference bind the exact dataset revision. `scripts/backfill-charts.py` is dry-run
by default; apply also requires `KAVEON_CHART_MIGRATION_ENABLED=true`. The 8 MiB
integrity-checked checkpoint advances by atomic replacement only after exact
owner-scoped reconciliation. No writer/outbox, scheduler, live run or cutover
exists, so PostgreSQL remains authoritative.

## Dashboard snapshot and shadow parity

The default-dry dashboard backfill captures PostgreSQL under `REPEATABLE READ,
READ ONLY` at the outbox watermark, with stable ID order, a 10,000-record bound
and 1 MiB canonical document limit. Layout, charts and filters must be valid JSON;
owners, visibility and chart IDs must be valid and duplicate chart references
are rejected. Favorites and thumbnail payloads are excluded.

Every referenced chart is read as the dashboard owner and must resolve at one
KaveonDB snapshot. The document stores a chart-revision map, and the Engine
derives typed chart references and rejects missing or stale revisions. Apply uses
exact owner-scoped create/reconciliation and requires
`KAVEON_DASHBOARD_MIGRATION_ENABLED=true`. The 8 MiB integrity checkpoint uses
atomic replacement and exact retry after ambiguous commits.

Set `KAVEON_DASHBOARD_SHADOW_READ_ENABLED=true` to compare authenticated point
reads. PostgreSQL still supplies the response. Telemetry contains only record ID,
status, hashes, sizes and target generation; request decorations and documents
are excluded. Filter-level dataset references, list parity, mutation outbox,
scheduling and cutover remain pending.

## DLM definition record

KaveonDB now accepts `kaveon.product.dlm_definitions` as a typed transactional
record kind. The record ID must equal its `dataset_id`, and the canonical JSON
object must contain exactly `dataset_id` and a positive `dataset_revision`.
KaveonDB derives a typed dataset reference, so the definition and dataset can be
created atomically and dataset deletion uses existing reference restrictions.
Owner isolation, compare-and-swap updates, immutable document storage and point
reads use the same product transaction boundary as other kinds. The API typed
client and replay family map recognize the kind, but no PostgreSQL DLM writer
emits it and no cutover is enabled.

Generated DLM manifests, answers, value indexes, routers and sketches are not
stored inside this definition. Their current tables represent a multi-record
derived generation with cache/retention semantics; moving them requires a
separate bounded atomic publication contract. No `dlm_run` kind is claimed by
the definition backfill.

KaveonDB now also has the small `dlm_run` metadata kind. A run document contains
exactly `definition_id`, positive `definition_revision`, `status` and `artifact`.
Creation is allowed only in `building` with a null artifact and only while the
referenced DLM definition exists at that exact revision. The sole update is
`building` to `ready` or `failed`: `ready` requires one normalized relative
manifest path and lowercase SHA-256, while `failed` retains a null artifact.
Both terminal states reject further updates. KaveonDB derives the typed
definition reference and applies existing owner isolation, CAS revisions,
immutable document persistence and delete restriction.

This record points to a committed manifest; it does not embed generated answers,
value indexes, sketches, cached results or error text. The manifest's own future
schema must reference immutable generated objects and be published before the
run becomes `ready`. No PostgreSQL writer, reader or cleanup process uses
`dlm_run` yet.

The bounded legacy-run backfill treats each ready PostgreSQL `dlm_artifact`
version as one historical run named `<dataset-id>-v<version>`. Capture uses one
`REPEATABLE READ, READ ONLY` source transaction, then reads every corresponding
owner-scoped KaveonDB definition at one target snapshot and binds its exact
record revision. Only `ready` rows with positive versions and valid JSON are
supported; every other status fails the whole capture.

Before capture, an operator must stage the canonical manifest JSON bytes at
`dlm/<dataset-id>/v<version>/manifest.json` under a local artifact root. Missing
or byte-divergent files fail before a checkpoint is created. The sealed run
contains that relative path and SHA-256; apply publishes `building` followed by
`ready` in one KaveonDB transaction.

```powershell
python scripts/backfill-dlm-runs.py `
  --checkpoint tmp/dlm-run-backfill.json `
  --artifact-root tmp/staged-dlm-artifacts

$env:KAVEON_DLM_RUN_MIGRATION_ENABLED = "true"
$env:KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED = "true"
python scripts/backfill-dlm-runs.py `
  --checkpoint tmp/dlm-run-backfill.json `
  --artifact-root tmp/staged-dlm-artifacts --resume --apply `
  --client-factory deployment_adls:create_immutable_client
```

Dry-run is the default. Apply requires both enable variables and an explicitly
injected client factory. The client contract exposes conditional create and a
bounded point read; it must never overwrite. Before target metadata is written,
the publisher rehashes the staged bytes, conditionally creates the remote path,
and reads it back exactly. A create error is treated as an ambiguous outcome and
accepted only when the remote bytes match; mismatch preserves the error.

The integrity-checked 4 MiB checkpoint holds at most
10,000 records and advances by atomic replacement after reconciliation. The
command is not scheduled or deployed. No concrete credential provider is built
into the repository, and no live publication has run; qualification still
requires a deployment-owned ADLS client and durable evidence.

## DLM migration rehearsal evidence

`scripts/collect-dlm-migration-evidence.py` builds a credential-free, canonical
evidence bundle from completed definition and run checkpoints, artifact
publication receipts, reconciliation reports, and owner-scoped target
observations. Collection is disabled unless
`KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED=true`. The bundle binds both PostgreSQL
watermarks and snapshot hashes, checkpoint file hashes, the dataset/definition
capture snapshots, every run's exact definition revision and artifact hash, and
the final KaveonDB snapshot plus per-record generations.

The verifier requires complete ID coverage, positive generations, verified
artifact receipts, matching counts and reconciliation hashes, a valid canonical
bundle SHA-256 and a caller-selected freshness window. Missing, stale, duplicate,
tampered or mismatched evidence fails closed. Sensitive field names are rejected,
and each input and the bundle are bounded at 4 MiB. This is rehearsal evidence;
it does not itself satisfy the broader DLM retirement gate or prove a deployed
writer, fencing, rollback, retention, backup/restore or production cutover.

The first deterministic definition backfill selects ready `dlm_artifact`
dataset IDs and owners inside one PostgreSQL `REPEATABLE READ, READ ONLY`
transaction at the product-outbox watermark. Because PostgreSQL has no KaveonDB
dataset revision, it then owner-reads every corresponding dataset and requires
all results to carry one KaveonDB snapshot ID before sealing definition hashes.
Missing datasets, invalid revisions or a changing target snapshot fail capture.

The operator command is dry-run by default:

```powershell
python scripts/backfill-dlm-definitions.py `
  --checkpoint tmp/dlm-definition-backfill.json

$env:KAVEON_DLM_DEFINITION_MIGRATION_ENABLED = "true"
python scripts/backfill-dlm-definitions.py `
  --checkpoint tmp/dlm-definition-backfill.json --resume --apply
```

The 4 MiB integrity-checked checkpoint stores at most 10,000 exact definitions,
the source watermark, target dataset snapshot ID and per-record/whole-snapshot
hashes. Progress advances through atomic replacement only after exact
owner-scoped reconciliation; a crash after target commit retries safely through
exact comparison. Apply requires both `--apply` and the enable variable. The
command is not scheduled or invoked by the API.

## Favorite migration

KaveonDB now accepts owner-isolated `favorite` records with a deterministic SHA-256 ID over owner, target kind and target ID. Dataset, chart, dashboard, saved-query and typed source favorites derive validated references and one owner-target unique value.

Favorite create/delete locks the owner-target PostgreSQL row and appends one canonical outbox event in the same transaction for supported targets. The default-dry backfill captures at most 100,000 records under repeatable read and maps a data-source target to `data:<id>`. Apply requires `KAVEON_FAVORITE_MIGRATION_ENABLED=true`; replay maps the family and `KAVEON_FAVORITE_SHADOW_READ_ENABLED=true` enables a 25-record owner-list comparison. PostgreSQL remains authoritative and the live outbox table remains absent.

## Public source authority boundary

KaveonDB now has a typed `source` product record for non-secret catalog and connection metadata. IDs are namespaced as `catalog:<id>` and `data:<id>`. Documents contain only source identity, display/catalog fields, lifecycle and an opaque `secret_ref`; the Engine exact-allowlists fields and rejects an absent reference. PostgreSQL connection strings, ciphertext, storage/adapter configuration and credentials are excluded from documents, checkpoints and reconciliation reports.

The default-dry coupled backfill captures `catalog_sources` and `data_sources` in one PostgreSQL repeatable-read snapshot at the outbox watermark, with a 10,000-record bound, deterministic hashes, checkpoint/resume and exact owner reconciliation. More than two rows claiming one catalog identity fails as ambiguous. Legacy connection rows receive only a `key-managed:data_sources/<id>` placeholder; a deployment-owned resolver backed by workload identity/Key Vault must replace and qualify it before cutover.

Catalog and data-source create/update/delete now lock mutable rows and commit the source mutation with exactly one canonical source event. Catalog audit entries share that transaction. Destructive operations restrict catalog identities still referenced by data sources and data sources still referenced by favorites. Outbox failure rolls the source write back, and lifecycle transitions reject state that changed before the row lock. Shared-source visibility differs from owner-isolated product reads, so shadow parity remains blocked on an explicit visibility contract. PostgreSQL and the encrypted connection column remain authoritative.

The API source-secret boundary reads only `KAVEON_KEY_VAULT_URL` and authenticates with `DefaultAzureCredential`, allowing the deployed workload identity to supply tokens without client secrets in configuration. Vault URLs must be HTTPS Azure Key Vault hosts with no path, and secret references must remain in the configured vault under `/secrets/<name>[/<version>]`. Source IDs produce deterministic SHA-256-based Key Vault-safe names. Set/get/delete enforce request, response and value bounds; external error bodies and credential values never enter raised errors. No live secret was created, read, deleted or migrated during qualification.

The runtime credential resolver recognizes the explicit versioned envelope `kaveon:keyvault:v1:<reference>` only when `KAVEON_SOURCE_SECRET_READ_ENABLED=true`. It resolves that reference through the bounded Key Vault boundary and never rewrites PostgreSQL. Disabled resolution and every authentication, transport, validation or Key Vault failure stop the connection attempt with a content-free credential error; there is no fallback to another credential. Existing Fernet envelopes and legacy migration behavior remain unchanged. No writer produces the Key Vault envelope yet, so activation requires a separately reconciled secret publication and PostgreSQL compare-and-swap operation.

## User recent migration

KaveonDB accepts an owner-isolated `user_recent` record for dataset, chart and dashboard targets. Its deterministic ID hashes owner and item identity; the document retains label, href, type and source ordering timestamp, derives an owner/item uniqueness value, and holds a typed target reference. The snapshot reads at one PostgreSQL repeatable-read watermark, caps total records at 200,000, rejects unsupported target types, and fails if any owner exceeds the source retention contract of 20 records. Exact owner-scoped reconciliation is idempotent after an ambiguous create response.

PostgreSQL add/update serializes on an owner advisory lock, applies retention, locks any eviction, and records the final create/update plus eviction tombstone in the same transaction. Owner clears are bounded by the 20-row invariant. Cross-owner target cleanup locks at most 101 rows and fails before mutation above its 100-event fanout bound. The tamper-evident checkpoint advances only after per-record reconciliation and requires `KAVEON_USER_RECENT_MIGRATION_ENABLED=true` for apply. `KAVEON_USER_RECENT_SHADOW_READ_ENABLED=true` compares at most 20 owner records without changing PostgreSQL responses. Live replay, parity, fencing and rollback evidence remain pending, so PostgreSQL stays authoritative.

## Query history migration

KaveonDB accepts an owner-isolated immutable `query_history` record keyed by the source UUID, with an optional typed dataset reference. The canonical document contains the SQL Lab fields needed to reproduce history views and excludes additive Engine detail telemetry. The deterministic retention policy keeps the newest 1,000 rows per owner by `executed_at DESC, id DESC`; the repeatable-read snapshot applies that window and caps migration at 100,000 documents. Enabled writers serialize per owner, emit at most one eviction tombstone after an append, and fail closed when legacy over-retention requires wider cleanup. Exact reconciliation handles ambiguous create outcomes.

The default-dry `scripts/backfill-query-history.py` command stores the source watermark, retained documents, hashes and next position in an atomic, 64 MiB-capped checkpoint. Apply requires `KAVEON_QUERY_HISTORY_MIGRATION_ENABLED=true` and advances only after exact per-record reconciliation. `KAVEON_QUERY_HISTORY_OUTBOX_ENABLED=true` makes append plus canonical events atomic and bounds owner deletion at 100 locked rows; `KAVEON_QUERY_HISTORY_SHADOW_READ_ENABLED=true` compares at most 50 owner rows. All controls default off. Live replay/parity, legacy retention cleanup, fencing and rollback evidence remain pending; PostgreSQL stays authoritative.

## Saved query migration controls

Saved-query create, update and delete retain their connection-pinned PostgreSQL transaction. Canonical outbox capture is now explicitly gated by `KAVEON_SAVED_QUERY_OUTBOX_ENABLED=true`, so repositories deployed before the outbox schema do not fail ordinary Studio writes. When enabled, mutation and event remain one unit of work and any outbox failure rolls back the source change. The existing repeatable-read backfill and tamper-evident checkpoint remain gated by `KAVEON_SAVED_QUERY_MIGRATION_ENABLED=true`. `KAVEON_SAVED_QUERY_SHADOW_READ_ENABLED=true` compares owner-scoped point reads and at most 25 list rows, returning hashes and counts without SQL content. All controls default off; PostgreSQL remains authoritative pending live reconciliation, parity, fencing and rollback evidence.

## Activity audit migration

KaveonDB accepts immutable `activity` records owned by the authenticated actor. The exact-allowlisted document preserves catalog audit identity, action, object fields, timestamp and structured details without creating a foreign-key dependency on an object that may later be deleted. Snapshot capture is repeatable-read and capped at 100,000 events; unstructured details or secret-shaped detail keys fail closed. The default-dry, 32 MiB-capped checkpoint applies only with `KAVEON_ACTIVITY_MIGRATION_ENABLED=true` and advances after exact reconciliation. Catalog audit insert plus outbox capture is atomic only when `KAVEON_ACTIVITY_OUTBOX_ENABLED=true`; `KAVEON_ACTIVITY_SHADOW_READ_ENABLED=true` compares at most 50 events as their original actors. All flags default off. Retention, workspace-wide target visibility, live parity and cutover remain pending; PostgreSQL stays authoritative.
