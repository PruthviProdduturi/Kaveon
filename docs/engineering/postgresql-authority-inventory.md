# PostgreSQL authority inventory

Status on September 10, 2026: PostgreSQL is the product metadata authority.
This inventory covers checked-in schema, runtime-created tables, and direct API
read/write paths. A KaveonDB protocol type is not evidence that its repository
family has migrated.

| PostgreSQL state | Current API readers/writers | KaveonDB destination | Blocking work |
| --- | --- | --- | --- |
| `catalog_sources` | `routers/catalog_sources.py`, Engine bridge, Lab and SQL routing | Native Engine catalog definitions plus future non-secret product source record | Unify source lifecycle at one pinned KaveonDB revision; retain only secret references; backfill and reconcile |
| `data_sources` | `routers/data_sources.py`, connection pool resolution, credentials service | Future non-secret source record | Separate encrypted connection envelope from public metadata; migrate favorites tied to sources |
| `datasets` | `services/datasets.py`; chat, AI, SQL and Lab readers | `dataset` product record | Source create/update/delete and one canonical outbox event now commit together; schema deployment, backfill, replay consumer and shadow reads remain |
| `dataset_dimensions`, `dataset_columns`, `dataset_metrics` | Dataset service; chat, query generator, AI and DLM readers | Children inside the revisioned `dataset` document | Source replacement is now atomic with its parent/outbox; target uniqueness/reference validation, backfill and reconciliation remain |
| `charts` | `services/charts.py`, dashboard rendering | `chart` product record | Define dataset reference extraction, stable legacy-ID mapping, outbox, backfill and visibility parity |
| `dashboards` | `services/dashboards.py`, DLM/dashboard routes | `dashboard` product record | Define chart/filter-dataset references at one revision; outbox, backfill and shadow rendering |
| `favorites` | `services/favorites.py`, dashboard and data-source routes | No typed favorite record yet | Add owner-unique favorite type and atomic dashboard/favorite behavior |
| `saved_queries` | `services/saved_queries.py` | `saved_query` product record | Outbox, backfill, owner/role parity and cutover |
| `user_themes` | `services/theme.py` | `user_theme` product record | Outbox, backfill and owner-key reconciliation |
| `user_recents` | `services/user_recents.py`, dashboard cleanup | No destination | Define bounded ordered personal-state record and retention |
| `query_history` | `services/query_history.py`, DLM usage | No destination | Partitioned append path, stable cursor ordering, retention and payload policy |
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
