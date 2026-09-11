# PostgreSQL retirement gate

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

The local parity audit fails closed over the complete checked-in authority
manifest:

```powershell
python scripts/audit-postgresql-retirement.py `
  --evidence tmp/postgresql-reconciliation-evidence.json `
  --output tmp/postgresql-retirement-audit.json `
  --max-age-hours 24
```

It requires all 16 authority families and exact table membership. Every family
must have fresh `passed` evidence for counts, stable IDs, ownership, references
and content hashes, matching source and target counts, a watermark, and a
lowercase SHA-256 report identity. The output binds the full input and audit to
SHA-256 digests without embedding credentials or row contents. Missing, stale,
future-dated, failed, duplicate, unknown or secret-shaped evidence returns a
nonzero exit code. This credential-free checker does not create reconciliation
evidence, discover live tables, enable reads or writes, fence PostgreSQL, or
authorize retirement.

Reconciliation jobs produce one strict, content-free JSON report per family.
The local collector verifies each report's canonical SHA-256 and exact family
identity before assembling the gate input:

```powershell
$env:KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED = "true"
python scripts/collect-postgresql-retirement-evidence.py `
  --reports tmp/reconciliation-reports `
  --output tmp/postgresql-reconciliation-evidence.json
```

Collection is disabled unless the environment variable is exactly `true` and
fails when any maintained family report is absent, oversized, malformed,
misnamed, tampered with or contains extra fields. Reports identify their
producer plus PostgreSQL and KaveonDB snapshot identities, but contain no rows,
credentials or connection details. The collector reads files only: it does not
connect to either system or turn an unexecuted reconciliation into evidence.
Run the parity audit separately to enforce freshness and all parity assertions.

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

A credential-free rehearsal bundle can now bind completed definition/run
checkpoints, artifact receipts, source watermarks, exact revision bindings,
KaveonDB snapshot/generations and reconciliation results. Its verifier rejects
stale or incomplete evidence. It is scoped DLM migration evidence and does not
replace the complete authority-family retirement gate.

The chart family now has deterministic snapshot, exact dataset-revision binding,
checkpoint/resume and target reconciliation code. Retirement remains blocked on
live backfill evidence, continuous mutation capture, read parity, fencing,
rollback and backup/restore qualification.

Dashboards now have snapshot/backfill/reconciliation and point-read shadow code,
including exact referenced chart revisions. Retirement remains blocked on typed
filter dataset references, continuous writer capture, live parity evidence,
fencing, rollback and backup/restore qualification.

Deleting the StatefulSet or PVC before these gates would remove the current
product metadata authority and break Studio even though ADLS analytical queries
remain available.
