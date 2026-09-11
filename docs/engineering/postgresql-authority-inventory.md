# PostgreSQL authority inventory

Status on September 10, 2026: PostgreSQL is the product metadata authority.
This inventory covers checked-in schema, runtime-created tables, and direct API
read/write paths. A KaveonDB protocol type is not evidence that its repository
family has migrated.

| PostgreSQL state | Current API readers/writers | KaveonDB destination | Blocking work |
| --- | --- | --- | --- |
| `catalog_sources` | `routers/catalog_sources.py`, Engine bridge, Lab and SQL routing | Typed non-secret `source` plus native Engine catalog definitions | Source mutation/outbox/audit atomicity exists; visibility parity, secret resolver and live evidence remain |
| `data_sources` | `routers/data_sources.py`, connection pool resolution, credentials service | Typed non-secret `source`; encrypted connection remains separate authority | Atomic metadata/outbox and default-off Key Vault envelope reads exist; reconciled secret publication/CAS, rotation, visibility parity and live evidence remain |
| `datasets` | `services/datasets.py`; chat, AI, SQL and Lab readers | `dataset` product record | Source create/update/delete and one canonical outbox event now commit together; schema deployment, backfill, replay consumer and shadow reads remain |
| `dataset_dimensions`, `dataset_columns`, `dataset_metrics` | Dataset service; chat, query generator, AI and DLM readers | Children inside the revisioned `dataset` document | Source replacement is now atomic with its parent/outbox; target uniqueness/reference validation, backfill and reconciliation remain |
| `charts` | `services/charts.py`, dashboard rendering | `chart` product record | Deterministic backfill and exact dataset revision binding exist; outbox, live reconciliation, write parity and cutover remain |
| `dashboards` | `services/dashboards.py`, DLM/dashboard routes | `dashboard` product record | Deterministic backfill, exact chart revision binding and point-read shadowing exist; filter-dataset references, outbox, live parity and cutover remain |
| `favorites` | `services/favorites.py`, dashboard and data-source routes | Typed owner-unique favorite for migrated product targets | Data-source destination, direct-route unification, live evidence and cutover remain |
| `saved_queries` | `services/saved_queries.py` | `saved_query` product record | Source mutations and outbox are atomic; deterministic backfill exists; shadow parity, live reconciliation and cutover remain |
| `user_themes` | `services/theme.py` | `user_theme` product record | Atomic source/outbox, bounded backfill and owner shadow code exist; outbox schema deployment, live replay/parity, fencing and cutover remain |
| `user_recents` | `services/user_recents.py`, dashboard cleanup | Typed owner-isolated `user_recent` with target reference | Atomic bounded writers, checkpointed reconciliation and default-off owner shadow exist; live replay/parity, fencing and rollback evidence remain |
| `query_history` | `services/query_history.py`, DLM usage | Typed owner-isolated `query_history`, optional dataset reference | Deterministic 1,000/owner retention, checkpointed backfill, default-off atomic events and shadow exist; live evidence and cutover remain |
| `activity` | Catalog-source audit paths | No destination | Immutable audit schema, retention and actor identity |
| `context_snapshots`, `context_answer_cache` | `dlm/profiler.py`, context routes | Rebuilt derived state | Define generation publication and cache retention; rebuild after dataset cutover |
| `dlm_artifact`, `dlm_value_index`, `dlm_router`, `dlm_answers`, `dlm_sketch` | `dlm/engine.py`, profiler/router and DLM routes | Typed `dlm_definition` for dataset/revision identity; generated state remains rebuilt | Backfill definitions, then publish a complete generated run atomically against that dataset revision; prevent stale routing |
| `chat_sessions`, `chat_messages` | `routers/chat_history.py`, `routers/chat.py`; created by `data/migrations/chat_history.sql` | No destination | Owner-scoped ordered append, atomic message/session update, encryption and deletion policy |
| `ai_providers`, `user_ai_keys` | `services/ai_service.py`; created at runtime | Key-managed secret boundary plus non-secret references | Keep encrypted keys outside ordinary product documents; define provider metadata authority and rotation references |

The PostgreSQL schema now includes `product_migration_outbox`. It is migration
infrastructure rather than a KaveonDB destination. `database.metadata.transaction`
pins one PostgreSQL connection and commits or rolls back all statements together.
`services.product_outbox.enqueue` writes a canonical, content-hashed event inside
that same transaction, assigns a monotonic source sequence, bounds payloads, and
rejects reuse of an event UUID with different content.

Dataset create/update/delete now use the unit of work and append exactly one
canonical event after all parent/child statements. Update locks the parent and
rejects a stale pre-lock revision; child or outbox failures roll the source
mutation back. Dataset deletion rejects dependent charts so PostgreSQL cannot
perform a cascade that the dataset event failed to capture. This code must not
be deployed before the outbox schema is applied. No replay process or KaveonDB
write is enabled yet.

The API contains a bounded ordered replay service, but no scheduler or operator
command invokes it. It validates each stored payload hash, applies as the
original record owner through the typed KaveonDB transaction client, and
acknowledges the locked PostgreSQL event only after target success. If the
target response is lost, replay accepts only an exact committed document; a
differing record fails closed. Deletes likewise resolve an already-absent
target. Failures record a bounded code and stop the batch before later source
sequences. This closes the application algorithm boundary, not its live
durability qualification.

Dataset backfill now has a deterministic implementation boundary. One
PostgreSQL `REPEATABLE READ, READ ONLY` transaction captures the outbox
watermark, parent rows and all semantic children in stable ID order. The
snapshot is bounded at 10,000 datasets, 1,000,000 rows per child family and 256
MiB of canonical JSON. Each record and the whole snapshot receive deterministic
SHA-256 identities. Applying a snapshot creates only missing KaveonDB records,
accepts an ambiguous response only after exact owner-scoped comparison, and
performs a second exact read of every record before returning a credential-free
reconciliation report. No command or scheduler invokes this code yet, and no
real PostgreSQL/KaveonDB report has been produced.

An operational command now exposes that bounded backfill with dry-run default,
an explicit apply enable variable, an integrity-checked exact-snapshot
checkpoint and per-record atomic checkpoint replacement. It is not invoked by
startup, an API route or a scheduler. The checkpoint contains customer metadata
and must remain in restricted ignored storage.

PostgreSQL retirement still requires a discovered live-schema report because
runtime and older deployments may contain tables absent from current source.
Backfill/replay must reconcile IDs, owners, visibility, references and canonical
payload hashes at a recorded source sequence. Cutover additionally requires a
write fence, zero outbox lag, role-filtered shadow-read parity, restart and
backup/restore evidence, and a tested rollback window.

`scripts/inventory-postgresql-authority.py` now produces that live-schema
report from one repeatable-read, read-only PostgreSQL snapshot. It enumerates
public base tables, fails before counting when any table is outside the 16-family
manifest and migration-infrastructure allowlist, records exact counts for
present authority tables, marks maintained but absent tables explicitly, and
binds the content-free report to SHA-256. The command has not yet run from the
deployed API image, so checked-in code is not live-schema evidence.

The credential-free parity gate in
`scripts/audit-postgresql-retirement.py` maps every table above into 16 explicit
authority families. Its checked-in manifest is the minimum coverage set: an
evidence file cannot omit a family, add an unknown family, change its table
membership or duplicate it. Each family must carry fresh successful checks for
counts, stable IDs, ownership, references and canonical content hashes, plus
matching source/target counts, a source watermark and the SHA-256 of its
underlying reconciliation report. Sensitive-shaped fields are rejected. The
gate reads local JSON only and does not discover schemas or access credentials,
so a separate read-only live-schema inventory must also prove that no
authoritative runtime table is absent from this maintained manifest.

`scripts/collect-postgresql-retirement-evidence.py` assembles that strict input
from one locally archived report per family. It is disabled by default, verifies
each report's canonical digest and binds its producer, PostgreSQL snapshot and
KaveonDB snapshot provenance. Its exact schema rejects row samples and arbitrary
fields. The collector is deliberately credential-free and performs no database
or Engine requests; the family reconcilers remain responsible for producing the
fresh read-only source/target facts.

The application dependency side of this inventory is machine checked with:

```powershell
python scripts/check-postgresql-dependencies.py `
  --output tmp/postgresql-cutover-dependencies.json
```

The scanner parses production Python database calls and matches their SQL table
references to the same 16-family manifest. Every observed family at a call site
must have an explicit `read`, `write` or conservative `read-write`
classification. It also rejects missing classified files, invalid access modes
and any family with no application call site. The emitted artifact contains
paths, access modes, families and table names only. A new database call that
mentions a maintained authority table fails until its cutover dependency is
classified. Dynamic SQL must still receive code review because static parsing
cannot infer a table name constructed entirely at runtime.

Datasets now have a first disabled shadow-read call site on authenticated point
reads. When explicitly enabled, it compares the canonical PostgreSQL dataset
document with one KaveonDB owner-scoped read under the requesting actor and role,
then emits hash/size/generation telemetry only. It never changes the returned
PostgreSQL object. Dataset lists now compare their response projection through
at most 25 owner-scoped target point reads; larger results skip target access,
and telemetry contains aggregate counts and batch hashes. Internal reads and
writes remain uncovered. This is a parity observation boundary, not a read
switch or cutover gate result.

Committed dataset mutations now also have a default-off verification observer.
It checks the exact durable outbox event first, classifies an unapplied event as
pending replay without reading KaveonDB, and compares only applied events under
the stored owner. The observer emits content-free telemetry and cannot change
the successful PostgreSQL response. It neither drives replay nor supplies live
evidence until explicitly enabled and observed in qualification.

Charts now have a separate default-off shadow comparator on authenticated point
reads. It uses one owner-scoped KaveonDB read under the requesting actor/role and
compares a fixed bounded projection, emitting hashes and status only while the
PostgreSQL response remains unchanged. Lists and mutations remain uncovered;
chart outbox, backfill and replay must precede any write observation or cutover.
A typed DLM definition destination now exists with exact dataset ID/revision
schema and a dataset reference. DLM shadowing remains blocked on a PostgreSQL
definition writer/backfill and on the separate atomic publication design for
generated runs; definitions do not absorb answer/value/sketch payloads.

A default-dry checkpointed DLM-definition backfill boundary now maps every ready
PostgreSQL `dlm_artifact` to the exact revision of its already-migrated dataset.
It rejects mixed KaveonDB dataset snapshots, missing owner-scoped datasets,
divergent definitions and checkpoint corruption. No live snapshot exists, no
writer emits definition outbox events, and generated DLM tables remain outside
this record, so PostgreSQL authority and readiness are unchanged.

Generated DLM state now has a minimal durable `dlm_run` metadata destination:
one definition-revision binding, lifecycle status and immutable manifest
path/hash. The Engine enforces `building` to one terminal state and owner-scoped
mutation/read access. Large answer/index/sketch payloads remain external and no
current PostgreSQL DLM writer publishes a run or its manifest, so this is a
destination contract rather than migration evidence.

A default-off run backfill now deterministically captures ready legacy artifact
versions, requires exact locally staged canonical manifests, binds exact
owner-scoped definition revisions, checkpoints progress and reconciles exact
target state. A create-only injected-client publisher now verifies staged hashes,
conditional creation and exact remote bytes before metadata apply. It has no
credential provider, scheduled execution or live evidence; PostgreSQL remains
authoritative.

The offline DLM rehearsal verifier can bind both completed checkpoints, source
watermarks, artifact receipts, exact definition revisions, KaveonDB
snapshot/generations and reconciliation results into one freshness-limited
bundle. It has only fixture evidence and does not change authority.

Charts now have a default-off deterministic snapshot/backfill boundary for both
supported PostgreSQL layouts. It binds owner-scoped chart records to exact
KaveonDB dataset revisions and has checkpoint/resume plus exact reconciliation.
No live snapshot, ongoing writer/outbox, fencing or cutover exists, so charts
remain PostgreSQL-authoritative.

Saved-query create, update and delete now append one canonical outbox event in
the same PostgreSQL transaction. Updates and deletes lock the owner-scoped row;
delete emits a tombstone. A deterministic repeatable-read backfill supports the
two maintained timestamp layouts, bounds records and documents, checkpoints
each applied record, and reconciles exact owner-scoped KaveonDB documents. It is
default-dry and has no live checkpoint, replay-lag, shadow-read, fencing or
rollback evidence, so saved queries remain PostgreSQL-authoritative.

Dashboards now have deterministic repeatable-read snapshot capture, exact
owner-scoped chart revision binding, bounded checkpoint/resume, exact target
reconciliation and a default-off authenticated point-read shadow comparator.
The canonical document excludes favorites and thumbnails. Filter-level dataset
references are not yet typed, mutations have no outbox, and no live backfill or
parity evidence exists; dashboards remain PostgreSQL-authoritative.

After datasets, DLM definitions/runs, charts and dashboards, the remaining
authority families with no complete destination/backfill path are catalog/data
sources, favorites, saved queries, themes, recents, query/activity/chat history,
context cache and AI provider/key configuration. Dataset, chart and dashboard
ongoing writers also still require outbox/replay coverage before any cutover.

User themes now have a single PostgreSQL mutation/outbox transaction, canonical
owner-keyed documents, deterministic checkpointed backfill, exact owner
reconciliation and default-off shadow reads. A September 11 live AKS read-only
probe confirmed `public.product_migration_outbox` is absent, so these writers
must not be enabled or deployed as migration-ready. No live backfill, replay,
parity, fencing or rollback evidence exists.

Favorites now have a typed owner-target unique record with validated references, atomic service mutations/outbox, deterministic checkpointed backfill, replay mapping and bounded list shadow parity. Data-source favorites remain PostgreSQL-only and fail migration capture because no typed source destination exists. The undeployed outbox schema, direct-route unification, live reconciliation, fencing and rollback remain blockers.

Catalog and data sources now have a typed non-secret destination plus a coupled deterministic backfill. Namespaced IDs prevent accidental row collision and catalog_identity exposes overlap; ambiguous identities fail closed. Data-source favorites map to the new source kind. Encrypted connection material remains PostgreSQL/key-management authority, and writer atomicity, shared visibility shadowing, secret resolver/rotation, live evidence, fencing and cutover remain open.
