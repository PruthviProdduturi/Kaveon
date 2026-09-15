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
workspace trail and other roles remain actor-scoped. `dlm_definitions` and DLM
runs are revision-bound KaveonDB records. In retirement mode, generation and
serving use create-only ADLS artifacts and do not fall back to the five legacy
PostgreSQL DLM tables.

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

Current status on September 14, 2026: the PostgreSQL-free runtime and migration
tooling are implemented locally, and the live AKS retirement gate has not passed.
**Do not delete or scale down PostgreSQL yet.**

The complete code-path and runtime-table audit is maintained in
[PostgreSQL authority inventory](postgresql-authority-inventory.md).

Kaveon's analytical payloads are in ADLS Gen2 and are queried through the
Engine catalog. Native table statistics and the product-record transaction
snapshot are also published to the dedicated `product-transactions` ADLS
container. The AKS storage account is HTTPS-only, hierarchical-namespace
enabled, denies anonymous access and shared-key authorization, defaults its
network firewall to deny, and grants the Engine workload identity scoped RBAC.

The code now gives all 16 declared authority families an explicit typed-record,
rebuild, or deletion disposition with deterministic migration and strict
evidence collection. Product-record families have bounded replay and direct
cutover reads/mutations; context cache is rebuilt, and legacy AI configuration
must be proven absent, securely migrated, or deliberately deleted. Dataset
semantics are committed atomically with their parent. DLM compiled context is
immutable in ADLS and definition/run metadata is revisioned in KaveonDB. Setup
and metadata-administration routes are fenced in retirement mode so Studio does
not probe an external metadata database.

Those implementation facts are not live retirement evidence. The active AKS
attempt must still produce fresh 16-family reconciliation, shadow parity across
roles and visibility states, a fixed final watermark with zero outbox lag, live
write-fence probes, PostgreSQL-unavailable restart, KaveonDB backup/restore, and
bounded rollback receipts from one immutable run. Until the final audit accepts
those artifacts, PostgreSQL remains the deployment authority and rollback
source.

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

After the first PostgreSQL-free API restart, run the API-image black-box smoke
probe through the same HTTP service used by Studio. Use a dedicated migrated
test identity that owns at least one saved query, recent, favorite, chat session
and query-history record, and choose a visible dataset with a ready DLM:

```powershell
$env:KAVEON_POSTGRESQL_FREE_SMOKE_ENABLED = "true"
$env:KAVEON_PROXY_SECRET = "<from the existing API secret>"
python -m services.postgresql_free_smoke_cli `
  --base-url http://kaveon-api.kaveon.svc.cluster.local:8080 `
  --identity retirement-probe@example.test `
  --dataset-id 7 `
  --question "show total sessions" `
  --output /retirement/receipts/postgresql-free-smoke.json
```

Plain HTTP is accepted only for loopback and Kubernetes service DNS; use
`--ca-cert` with HTTPS. The command authenticates through the trusted proxy
header contract and probes health, source discovery, list and point reads,
user-owned product state, DLM artifact/context/ask, and chat serving. It fails
on empty required fixture families. Its receipt contains endpoint names,
statuses, counts and canonical state digests only; it never writes the proxy
secret, identity-bearing rows, SQL, questions, chat answers, or response bodies.

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

### Lossless seven-family publication

The reviewed canonical baseline produced by
`postgresql_baseline_identity` is the only accepted input for the two context
tables and five DLM tables. Run the publisher in the API workload-identity pod;
it reads `KAVEON_ADLS_ACCOUNT` and `KAVEON_ADLS_CONTAINER` and never accepts a
storage key or token on the command line.

```powershell
$env:KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED = "true"
python -m services.postgresql_special_family_migration_cli `
  --baseline /retirement/postgresql-special-family-baseline.json `
  --prefix retirement/special-families/<immutable-run-id> `
  --expected-head-etag absent `
  --output /retirement/special-family-migration.json
```

For a later commit, pass the exact quoted ETag returned for `head.json`. The
command creates and reads back seven immutable table objects, writes and reads
back the immutable manifest, then updates `head.json` with `If-None-Match: *` or
`If-Match`. A retry is accepted only when the existing object bytes and final
head pointer are identical. A different object or head fails closed. Preserve
the baseline and emitted evidence, then provide both to
`retire-postgresql-special-families.py` as `--lossless-baseline` and
`--lossless-migration-evidence`; retirement recomputes their exact identities
inside the locked deletion transaction.

## Implemented boundary and remaining live proof

All 16 authority families have checked-in reconciliation producers and a strict
collector. Runtime read authority is exact and fail-closed. Direct KaveonDB
mutations cover datasets and semantic children, charts, dashboards, saved
queries, themes, recents, favorites, query history, activity, sources, chat
sessions/messages and DLM definition/run records. Source secrets remain in Key
Vault and only validated versioned references enter product records.

DLM generation assigns a bounded next version, publishes canonical bytes
create-only to ADLS, verifies them, and atomically commits the dataset-bound
definition plus `building` to `ready` run transition. Serving reads routing,
values, answers, charts, coverage and curation from that verified artifact.
Request caches are actor-scoped and artifact-version/SHA-256-bound. Legacy mode
keeps the existing PostgreSQL and outbox behavior.

Retirement and restart-rehearsal modes make `/setup/status` report KaveonDB as
configured. External metadata connection test/update, setup initialization,
start-fresh and legacy repair routes return `409 postgresql_retired` before pool
or configuration access.

None of these code paths proves the active deployment is ready. The remaining
work is operational: collect fresh reports from the deployed image, prove exact
shadow parity, fence and drain one final watermark, restart with PostgreSQL
unavailable, restore KaveonDB from its immutable backup, rehearse bounded
rollback, and pass the final evidence runner with exactly 16 families. Scale
PostgreSQL to zero only after review; retain its PVC and snapshot for the agreed
rollback window. Delete them only after that window closes.
