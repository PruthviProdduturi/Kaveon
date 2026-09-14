# PostgreSQL retirement gate

## Family-scoped read authority

`KAVEONDB_READ_AUTHORITY_FAMILIES` is empty by default, so PostgreSQL remains
the read authority. Point reads for `datasets`, `charts`, and `dashboards` can
be moved independently with a comma-separated value such as
`datasets,charts`. A selected family reads only KaveonDB: target errors and
invalid documents fail closed and never fall back to PostgreSQL. Unknown family
names reject the configuration. The API reapplies visibility rules after its
privileged Engine bridge read.

Do not select a family until its reconciliation and role-based shadow reads
pass at the final source watermark. Point and list reads then use KaveonDB only.
Lists traverse bounded, snapshot-pinned pages, apply the legacy visibility
rules, and return records in descending modification order. A snapshot change
during pagination fails the request instead of mixing catalog generations.
Favorite state is joined from KaveonDB's owner-scoped favorite records, so
cutover lists do not query PostgreSQL for presentation metadata.

The same switch accepts `saved_queries`, `user_themes`, `user_recents`,
`favorites`, `query_history`, `chat_history`, and `sources`. Personal families
are owner-filtered after the privileged bridge read. Chat sessions and messages
remain separate typed records and are joined by session ID in the API. Source
responses expose only the credential-free public projection stored in KaveonDB.
Catalog-source list and point reads use the same family switch. Their KaveonDB
documents include storage and adapter configuration, lifecycle, ownership, and
timestamps. Credentials remain indirect: secret-backed sources expose only a
validated Key Vault URI, while managed identities expose a bounded principal
reference. Raw credentials and connection strings are rejected by migration
and by the Engine document validator.
Source mutations require `sources,activity` to move together. Catalog and data
source metadata then writes directly to KaveonDB with an activity record in the
same revision-CAS transaction. Data-source connection strings are written to
Key Vault first and only the validated versioned URI enters KaveonDB; failed
creates compensate by deleting that secret. Activity-only writes also bypass
PostgreSQL when `activity` is selected. Malformed target records, missing audit
authority, and concurrent revisions fail closed.
When `user_themes` has read authority, theme create, update, and delete also use
KaveonDB directly. Updates and deletes bind the exact current revision; missing
deletes are idempotent, malformed revisions fail closed, and no operation falls
back to PostgreSQL.
The same mutation cutover is implemented for `saved_queries`, `favorites`, and
`user_recents`. Saved-query updates and deletes use revision CAS. Favorites use
an owner/type/object deterministic identity, including normalization of legacy
`data_source` references to non-secret `source` records. Recent upserts and the
20-item retention delete commit together; cross-owner deletion is limited to
100 records and commits separately under each record owner. Invalid documents,
revisions, ownership, or fanout fail closed without querying PostgreSQL.
Catalog audit reads can independently select `activity`; administrators see the
workspace trail and other roles remain actor-scoped. `dlm_definitions` is
reserved in the allowlist but the existing DLM status endpoint cannot select it
until the compiled artifact is retrievable without PostgreSQL; enabling that
name alone therefore does not claim DLM read cutover.

## Context cache retirement

`context_snapshots` and `context_answer_cache` are revision-bound generated
state. They are rebuilt rather than copied: copying answer rows would retain
possibly sensitive results and could make stale dependencies authoritative.
The deployment-owned retirement job must fence profiler/router cache writes,
capture only row counts and schema digests, rebuild against the exact cutover
dataset revision, repeat a deterministic probe, delete the old PostgreSQL rows,
and verify both tables are empty. Generate the family report with:

```powershell
$env:KAVEON_CONTEXT_CACHE_RETIREMENT_ENABLED = "true"
python scripts/build-context-cache-retirement-report.py `
  --evidence tmp/context-cache-retirement-observation.json `
  --output tmp/postgresql-retirement-reports/context_cache.json
```

The observation is bounded to 256 KiB and rejects fields whose names indicate
questions, answers, results, SQL, profile values, credentials, or secrets. The
report is emitted only when rebuild coverage is complete, repeated probe hashes
match, writes are fenced, deletion counts match the pre-deletion inventory, and
zero rows remain. This report is one family input; it does not bypass the other
authority-family or global retirement gates.

Status on September 10, 2026: **do not delete or scale down PostgreSQL**.

The complete code-path and runtime-table audit is maintained in
[PostgreSQL authority inventory](postgresql-authority-inventory.md).

Kaveon's analytical payloads are in ADLS Gen2 and are queried through the
Engine catalog. Native table statistics and the product-record transaction
snapshot are also published to the dedicated `product-transactions` ADLS
container. The AKS storage account is HTTPS-only, hierarchical-namespace
enabled, denies anonymous access and shared-key authorization, defaults its
network firewall to deny, and grants the Engine workload identity scoped RBAC.

That does not mean every product record has moved. The API still treats the AKS
PostgreSQL database as the authority for Studio metadata. A read-only inventory
on September 10 found these 21 live tables:

| Table | Rows | Table | Rows |
|---|---:|---|---:|
| activity | 1 | catalog_sources | 2 |
| charts | 70 | context_answer_cache | 0 |
| context_snapshots | 0 | dashboards | 8 |
| data_sources | 0 | dataset_columns | 119 |
| dataset_dimensions | 0 | dataset_metrics | 35 |
| datasets | 9 | dlm_answers | 3,659 |
| dlm_artifact | 9 | dlm_router | 9 |
| dlm_sketch | 0 | dlm_value_index | 0 |
| favorites | 0 | query_history | 660 |
| saved_queries | 0 | user_recents | 15 |
| user_themes | 0 |  |  |

The ADLS product-record layer currently has typed records for datasets, charts,
dashboards, saved queries and user themes. The API now has a typed, bounded
client for atomic create/update/delete groups and owner-scoped point reads. It
is an application migration boundary, not an enabled repository adapter:
backfill, PostgreSQL outbox/units of work, shadow-read wiring, write fencing and
API cutover remain incomplete. It does not yet cover all 21 PostgreSQL tables.

The PostgreSQL schema and API contain the source-side unit-of-work and
idempotent migration-outbox primitives. The dataset parent and semantic-child
writes now append exactly one event in that transaction. This is source capture,
not migration completion: the schema is not deployed and no replay, backfill,
reconciliation, shadow read or cutover is enabled.

A bounded replay library now processes source sequence order and resolves an
ambiguous target response only from exact committed KaveonDB content. It is not
wired to a scheduler or deployment, and no backfill watermark exists. Therefore
it provides no evidence that current PostgreSQL rows are present in KaveonDB.

A deterministic dataset snapshot/backfill library now records a repeatable-read
source watermark, canonical record and snapshot hashes, bounded counts, and
exact post-create target reconciliation. It is disabled operationally and has
not run against a real environment, so the required backfill and reconciliation
evidence remains absent.

The backfill has a disabled-by-default command with an exact snapshot checkpoint
and resume support. Its existence is not a retirement gate result; only an
archived successful command report followed by post-watermark replay and final
reconciliation can supply that evidence.

### Durable backfill checkpoints

Every `scripts/backfill-*.py` command publishes each per-record checkpoint to
ADLS before processing the next record. Deployed apply runs fail closed unless
the durable backend is configured:

```powershell
$env:KAVEON_MIGRATION_CHECKPOINT_MODE = "adls"
$env:KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT = "<storage-account>"
$env:KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER = "<private-container>"
$env:KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX = "retirement/<immutable-run-id>"
python scripts/backfill-product-catalog.py `
  --checkpoint tmp/datasets.json --apply --resume
```

The workload identity needs blob read, create, and update permissions on that
container. The active blob uses ETag compare-and-swap, so two writers cannot
advance one migration silently. Each state is also retained under a
content-addressed version key, and every active write is read back byte-for-byte
before the next source record runs. Existing checkpoint SHA-256 validation
still applies after hydration. A local checkpoint without its corresponding
ADLS object is rejected rather than promoted implicitly.

Filesystem-only apply is available solely for an explicit local environment:
set `KAVEON_MIGRATION_CHECKPOINT_MODE=local` and
`KAVEON_ENVIRONMENT=local`. It is not valid retirement evidence. Dry runs may
continue to use local checkpoints without cloud credentials.

The local parity audit fails closed over the complete checked-in authority
manifest:

```powershell
python scripts/audit-postgresql-retirement.py `
  --evidence tmp/postgresql-reconciliation-evidence.json `
  --output tmp/postgresql-retirement-audit.json `
  --max-age-hours 24
```

It requires all 16 declared authority families and exact table membership. Every family
must have fresh `passed` evidence for counts, stable IDs, ownership, references
and content hashes, matching source and target counts, a watermark, and a
lowercase SHA-256 report identity. The output binds the full input and audit to
SHA-256 digests without embedding credentials or row contents. Missing, stale,
future-dated, failed, duplicate, unknown or secret-shaped evidence returns a
nonzero exit code. This credential-free checker does not create reconciliation
evidence, discover live tables, enable reads or writes, fence PostgreSQL, or
authorize retirement.

For a live migration rehearsal, use the combined runner after the read-only
reconciliation jobs have written every family report and `retirement-gates.json`.
Pin `--now` to the run's UTC checkpoint so the evidence and freshness decision
are reproducible:

```powershell
$env:KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED = "true"
python scripts/run-postgresql-retirement-evidence.py `
  --reports tmp/reconciliation-reports `
  --evidence tmp/postgresql-reconciliation-evidence.json `
  --audit tmp/postgresql-retirement-audit.json `
  --now 2026-09-10T20:00:00Z `
  --max-age-hours 24
```

The runner reads only local, content-bound reports. It requires all authority
families plus source watermark, zero outbox lag, write-fence, shadow-parity,
restart/recovery, rollback and backup/restore evidence. It writes outputs only
after validation; a missing, stale, failed or malformed input returns a
nonzero exit code. It never connects to PostgreSQL, changes the write fence,
scales a workload, or authorizes retirement.

After the read-only evidence runner, produce the final qualification summary:

```powershell
python scripts/retirement-qualification-summary.py `
  --audit tmp/postgresql-retirement-audit.json `
  --operational tmp/postgresql-operational-rehearsals.json `
  --output tmp/postgresql-retirement-summary.json
```

The summary is fail-closed and reports every declared authority family and all
seven global gates, plus separate backup/restore, rollback,
PostgreSQL-unavailable restart, and durable-checkpoint rehearsal gates. It
returns exit code 2 while any evidence is missing. The `ai_configuration`
family covers legacy `ai_providers` and `user_ai_keys` tables. Their former
application service is gone, but retirement still requires evidence that they
are absent, securely migrated, or deliberately deleted. Secret values must
never be placed in ordinary product records or retirement evidence.

Reconciliation jobs produce one strict, content-free JSON report per family.
After every backfill job has uploaded its final checkpoint, run the API-image
collector against locally hydrated copies of those durable checkpoints and the
live Engine endpoint. The collector performs read-only, snapshot-pinned target
enumeration and refuses missing or extra records as well as content drift:

```powershell
$env:KAVEON_RECONCILIATION_REPORT_COLLECTION_ENABLED = "true"
python -m services.postgresql_reconciliation_report_cli `
  --manifest /evidence/reconciliation-manifest.json `
  --output-directory /evidence/family-reports
```

The manifest has exactly four fields: `schema_version` (`1`), `checkpoints`,
`live_inventory`, and `special_reports`. `checkpoints` must name completed,
tamper-evident files for `datasets`, `charts`, `dashboards`, `favorites`,
`saved_queries`, `user_themes`, `user_recents`, `query_history`, `activity`,
`sources`, `chat_history`, `dlm_definitions`, and `dlm_runs`. Paths are relative
to the manifest unless absolute. `special_reports` must name independently
verified `context_cache` and `dlm_generation` reports.

The command derives `dataset_semantics` only from semantic children embedded in
the exactly matched dataset documents. It emits an `ai_configuration` report
only when the integrity-bound live inventory shows zero rows in both legacy AI
tables. The context-cache report must already prove deterministic rebuild,
write fencing, deletion, and zero remaining rows. DLM generation requires both
definition and run checkpoints to match live KaveonDB plus its separate report;
the command never turns those two checkpoints into evidence for generated
answers, routers, sketches, or value indexes. It creates the output directory
atomically only after all 16 families pass.

The API image contains the two special-family report commands used by the
optional `api.retirementReports` Helm Jobs. Mount a restricted evidence PVC and
place the live observations on it before enabling the Jobs:

```powershell
python -m services.context_cache_retirement_cli `
  --evidence /evidence/context-cache-live.json `
  --output /evidence/reports/context_cache.json --max-age-hours 1

python -m services.dlm_generation_retirement_cli `
  --bundle /evidence/dlm-migration-bundle.json `
  --retirement-evidence /evidence/dlm-generation-live.json `
  --output /evidence/reports/dlm_generation.json --max-age-hours 1
```

`dlm-generation-live.json` must identify one PostgreSQL snapshot and watermark,
give exact row counts and schema SHA-256 values for all five DLM authority
tables, and provide a later deletion observation containing the active write
fence, exact deleted counts, zero remaining counts, verification time, and the
KaveonDB target snapshot ID. That target ID must equal the compiled-artifact
bundle's independently observed snapshot. The context report retains its
existing requirement for source schema identities, complete deterministic
dataset rebuild coverage, and exact post-fence deletion. Both commands are
disabled unless their dedicated enable variables equal `true`; neither command
executes deletion or fills missing evidence fields.

The local collector verifies each report's canonical SHA-256 and exact family
identity before assembling the gate input:

```powershell
$env:KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED = "true"
python scripts/collect-postgresql-retirement-evidence.py `
  --reports tmp/reconciliation-reports `
  --output tmp/postgresql-reconciliation-evidence.json
```

The reports directory must also contain `retirement-gates.json`. The gate file
must contain fresh, machine-readable evidence for the source watermark and
zero pending outbox events, an active PostgreSQL write fence, complete shadow
parity, restart recovery, rollback, and a backup identity whose restore has
been verified. The retirement verifier fails closed when any gate is missing,
stale, false, malformed, or unverifiable; a family reconciliation report by
itself cannot authorize retirement.

Collection is disabled unless the environment variable is exactly `true` and
fails when any maintained family report is absent, oversized, malformed,
misnamed, tampered with or contains extra fields. Reports identify their
producer plus PostgreSQL and KaveonDB snapshot identities, but contain no rows,
credentials or connection details. The collector reads files only: it does not
connect to either system or turn an unexecuted reconciliation into evidence.
Run the parity audit separately to enforce freshness and all parity assertions.

During the final watermark drain, enable the source write fence on every API
replica and migration worker:

```powershell
$env:KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED = "true"
```

The fence is enforced inside the metadata query boundary, before PostgreSQL
pool execution and before transaction cursors are opened. It rejects mutations
of every table in the 16-family authority manifest while allowing read-only
reconciliation and outbox-drain bookkeeping. The flag defaults off and only
the exact value `true` enables it. A passing `write_fence` evidence record must
come from a live probe that observes rejected create, update and delete paths;
setting the variable or passing unit tests alone is not retirement evidence.

PostgreSQL may be retired only after one repeatable migration command proves all
of the following against a preserved backup:

1. Every authoritative table has an ADLS/KaveonDB destination or an explicit,
   reviewed retirement decision.
2. Counts, stable identifiers, ownership, references and content hashes
   reconcile between PostgreSQL and the committed KaveonDB snapshot.
3. API reads run against KaveonDB in shadow mode and match PostgreSQL.
4. A write fence stops PostgreSQL drift; create, update, delete and read paths
   then run only through KaveonDB.
5. API and Studio restart successfully with PostgreSQL unavailable, and the
   eight dashboards, 70 charts, nine datasets and nine DLMs still work.
6. Conflict, rollback, backup/restore and coordinator-restart tests pass, with
   credential-free machine-readable reports.
7. PostgreSQL is first scaled to zero while its PVC is retained. Permanent
   deletion happens only after the rollback window passes.

The AKS cluster has the repository-owned `kaveon-azuredisk-retain`
`VolumeSnapshotClass` (`disk.csi.azure.com`, incremental, `Retain`). Before the
schema/cutover rehearsal, fence application writes, force a PostgreSQL
checkpoint, create a `VolumeSnapshot` of `data-kaveon-postgres-0`, and wait for
`readyToUse=true`. Record its bound content and Azure snapshot identity in the
rehearsal evidence. Do not treat a snapshot taken during active writes as the
preserved retirement backup.

Passing the parity audit supplies evidence only for item 2. Items 1 and 3-7
remain independent mandatory gates, including live-schema discovery, shadow
reads, write fencing, restart, backup/restore and rollback qualification.

The checked-in cutover dependency inventory (`python
scripts/check-postgresql-dependencies.py`) must also pass before item 1 can be
reviewed. It maps production PostgreSQL query/execute call sites to all 16
authority families and their read/write role, failing when an observed table
family is unclassified. This is source-code coverage rather than proof that a
deployment has no older or dynamically constructed authority path; preserve the
separate live-schema and runtime qualification gates.

Generate the separate exact live-schema artifact from the same deployed API
image used for migration:

```powershell
python scripts/inventory-postgresql-authority.py `
  --output tmp/postgresql-live-authority.json
```

This command runs in one repeatable-read, read-only transaction and fails on an
unclassified public table. It contains table names, exact counts, presence,
family mapping, source-snapshot identity and a report digest; it contains no
rows or credentials. Archive it with the cutover evidence. A locally generated
fixture report does not qualify the live AKS schema.

The dataset shadow comparator is disabled by default and covers authenticated
point reads plus list projections of at most 25 records. Larger lists skip all
target access. Enabling it emits bounded hashes and aggregate parity status while
PostgreSQL still supplies the response. Retirement requires fresh
aggregate evidence across representative roles, visibility states and changes;
the existence of this comparator does not satisfy the shadow-read gate for
datasets or any other family.

The default-off dataset post-write observer distinguishes unapplied outbox lag
(`pending_replay`) from a changed/missing source event or applied target
divergence. It verifies applied events with owner-scoped reads and never changes
the PostgreSQL mutation result. This closes a telemetry boundary only; replay,
zero-lag fencing, live mutation coverage and durable report aggregation remain
required before dataset cutover.

Authenticated chart point reads also have a default-off bounded shadow
comparator. This establishes only hash telemetry for one read path. Chart list
coverage, source capture, backfill, replay, write verification and live parity
evidence remain mandatory, and the comparator cannot satisfy the chart family
retirement gate by itself.

KaveonDB now has a durable owner-isolated `dlm_definition` record containing
only dataset identity and positive pinned revision, with a typed dataset
reference. No PostgreSQL writer/backfill uses it yet. Generated DLM artifacts,
answers, indexes, router and sketches still lack a bounded atomic generation
contract, so the DLM family remains PostgreSQL-authoritative/rebuilt state and
cannot pass retirement.

A deterministic, default-dry definition backfill command now exists with exact
checkpoint/resume and owner-scoped reconciliation. It depends on datasets having
already reached a stable KaveonDB snapshot and rejects mixed target generations.
It has not run against a live environment, does not capture ongoing definition
writes, and does not migrate generated runs; it supplies no DLM retirement
evidence by itself.

The durable `dlm_run` record now binds a generated run to one exact definition
revision and immutable manifest identity, with a one-way building/ready/failed
lifecycle. It stores no answer payloads or error text. PostgreSQL-generated
artifacts still lack a manifest publisher, backfill, reconciliation and cleanup
policy, so the new metadata contract does not advance the DLM retirement gate.

The credential-free legacy-run command supplies deterministic snapshot,
checkpoint/resume and exact reconciliation behavior. Its injected create-only
publisher reconciles ambiguous writes from exact remote bytes, but production
use remains blocked on a deployed ADLS client and provenance for every sealed
path/hash.

Live DLM generation now has a retirement-mode commit path. It assigns the next
run version from a bounded KaveonDB snapshot, publishes canonical compiled bytes
create-only to ADLS and verifies them, then atomically creates or revision-CAS
updates the dataset-bound definition and advances the run from `building` to
`ready`. Only the dataset owner may publish. PostgreSQL mode retains the source
transaction and outbox path. Immutable bytes deliberately precede the KaveonDB
transaction, so a failed metadata commit can leave an unreferenced object but
can never expose a ready run whose bytes are missing or divergent.

The retirement compiler holds value-index rows, router terms and precomputed
answers in request-local bounded state rather than writing its five legacy DLM
tables. Native source discovery and all build scans use the Engine bridge. The
state, including preserved human curation, is sealed into the immutable compiled
artifact before the KaveonDB run becomes ready. Context edits use the same
create-only artifact and new-run path. The non-retirement compiler continues to
use its existing metadata tables and outbox contract.

A credential-free rehearsal bundle can now bind completed definition/run
checkpoints, artifact receipts, source watermarks, exact revision bindings,
KaveonDB snapshot/generations and reconciliation results. Its verifier rejects
stale or incomplete evidence. It is scoped DLM migration evidence and does not
replace the complete authority-family retirement gate.

The chart family now has deterministic snapshot, exact dataset-revision binding,
checkpoint/resume and target reconciliation code. Retirement remains blocked on
live backfill evidence, continuous mutation capture, read parity, fencing,
rollback and backup/restore qualification.

User themes now have atomic source/outbox writes, bounded checkpointed backfill,
exact owner reconciliation and shadow parity code. The source outbox table is
absent in the live AKS PostgreSQL deployment as verified by a read-only
`to_regclass('public.product_migration_outbox')` probe on September 11, 2026.
Deployment, replay, live evidence, fencing and rollback remain required.

The saved-query family now has atomic source mutation/outbox capture plus a
bounded deterministic backfill with exact owner-scoped reconciliation and
tamper-evident checkpoint/resume. The command is disabled for apply by default.
No live backfill, post-watermark replay, shadow-read, fence, restart or rollback
evidence exists, so this family does not yet satisfy a retirement gate.

Dashboards now have snapshot/backfill/reconciliation and point-read shadow code,
including exact referenced chart revisions. Retirement remains blocked on typed
filter dataset references, continuous writer capture, live parity evidence,
fencing, rollback and backup/restore qualification.

Deleting the StatefulSet or PVC before these gates would remove the current
product metadata authority and break Studio even though ADLS analytical queries
remain available.

In an evidence-qualified retirement or restart-rehearsal process, the setup
status route reports KaveonDB as the configured product authority without
probing an external metadata database. The administrative metadata connection
test/update, setup initialization, start-fresh, and legacy data-source repair
routes return `409 postgresql_retired` before opening a connection or changing
configuration. This fence is part of PostgreSQL-free operation; operators must
leave retirement mode and requalify the activation evidence before restoring an
external metadata authority.

Favorites now cover typed migrated targets with owner uniqueness, references,
source/outbox atomicity, backfill/replay, shadow code, and direct KaveonDB
mutations. Legacy data-source favorites normalize to the non-secret typed source
destination. Live reconciliation, fencing, and rollback evidence remain gates.

Source retirement now has a non-secret typed destination and deterministic coupled backfill, but encrypted connection material remains a separate secret authority. A workload-identity Key Vault resolver, writer/outbox atomicity, shared visibility semantics, shadow parity, rotation, backup/restore and rollback evidence are mandatory before retirement.
