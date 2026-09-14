# PostgreSQL retirement operational evidence

PostgreSQL retirement requires eight independently observed rehearsal receipts.
The collector does not run infrastructure commands and cannot turn missing
observations into passing evidence. Keep raw command output in the restricted
operations archive; checked-in or retirement-gate JSON contains only counts,
resource/revision identifiers, booleans, and SHA-256 identities.

Place these integrity-bound receipts in one run directory:

| Receipt | Required observation |
|---|---|
| `source_watermark.json` | Source snapshot SHA-256 and its observed watermark |
| `outbox_drain.json` | Bounded query ID, watermark, pending count before, and zero after |
| `write_fence.json` | Deployment revision, successful read probe, and one rejected mutation probe for every authority family |
| `shadow_parity.json` | Source/target snapshot identities, zero mismatches, and all 16 families exactly once |
| `restart_recovery.json` | PostgreSQL unavailable, new API and Studio processes ready, nonzero probes, and identical content-free state digests/counts before/after |
| `rollback.json` | Cutover revision, target fenced, source reads/writes restored within time and operation bounds, and identical state digests |
| `backup_identity.json` | Immutable ADLS prefix and manifest digest, an executed restore job, positive restored table count, and identical source/restored inventory digests |
| `durable_checkpoint.json` | Different pod UIDs, identical checkpoint digest, non-regressing position, and completed resume |

Each JSON object has exactly these top-level fields:

```json
{
  "schema_version": 2,
  "gate": "restart_recovery",
  "checked_at": "2026-09-14T18:00:00Z",
  "evidence_id": "aks-run-20260914-restart",
  "details": {"verified": true},
  "observation": {},
  "receipt_sha256": "lowercase SHA-256 of the canonical object without this field"
}
```

The exact gate-specific observation fields are defined in
`api/services/postgresql_operational_evidence.py`. Unknown fields, secret-shaped
keys, oversized files, duplicate/missing family probes, mismatched state
digests, reused pod UIDs, unbounded rollback time, and unverified restore claims
all fail closed.

State inventories must contain only `kind`, `id`, `revision`, and the committed
document SHA-256. `kaveondb_recovery_evidence.state_identity` rejects payloads,
duplicates, invalid revisions, and more than one million records before deriving
the before/after identity. Its backup validator accepts only query-free ADLS
paths containing `/backups/<backup-id>/`, exact object metadata, and immutable
object digests. Validating this manifest proves backup identity only. The
`backup_identity` receipt additionally requires `restore_executed: true`, a
restore job ID, and a matching restored inventory; never set these from a plan
or an unexecuted command.

Run the create-only rehearsal against a new `/restores/<job-id>/` prefix. The
backup must include `state-inventory.json` and list its exact size, ETag, and
SHA-256 in the immutable manifest. The command copies each listed object with
`If-None-Match: *`, reads it back, recomputes the content-free state identity,
and prints the strict `backup_identity` observation only after success:

Create that immutable backup first from the active product-transaction prefix.
The producer pins one Engine product snapshot across all typed families,
enumerates the ADLS prefix before and after copying, caps the run at 10,000
objects and 2 GiB, and publishes `manifest.json` last with create-only writes.
The local manifest is content-free and is the input to the restore rehearsal:

```powershell
python scripts/create-kaveondb-adls-backup.py `
  --account ACCOUNT --container CONTAINER `
  --active-prefix PRODUCT_TRANSACTION_PREFIX `
  --backup-id RUN_ID `
  --manifest-output tmp/kaveondb-backup-manifest.json
```

```powershell
python scripts/rehearse-kaveondb-adls-restore.py restore `
  --manifest tmp/kaveondb-backup-manifest.json `
  --restore-prefix https://ACCOUNT.dfs.core.windows.net/CONTAINER/restores/RUN_ID/ `
  --cleanup-manifest tmp/kaveondb-restore-cleanup.json
```

Cleanup is deliberately separate and binds every delete to the ETag returned
by the rehearsal create. Review and archive the successful observation first:

```powershell
python scripts/rehearse-kaveondb-adls-restore.py cleanup `
  --cleanup-manifest tmp/kaveondb-restore-cleanup.json
```

### Live probe recorder

Use `record-postgresql-operational-evidence.py` to execute the rehearsal probes.
Its manifest contains exactly one argument array and timeout for every gate. It
uses direct process execution (`shell=False`), bounds arguments, timeouts,
stdout and stderr, requires exit code zero and an exact JSON observation, then
derives the gate details from that observation. It publishes no receipts unless
all eight probes succeed and validate.

The source watermark and outbox entries can call the included read-only probe
inside the API workload:

```json
{
  "schema_version": 1,
  "run_id": "aks-retirement-20260914-01",
  "probes": {
    "source_watermark": {
      "argv": ["python", "scripts/probe-postgresql-source-state.py", "--gate", "source_watermark"],
      "timeout_seconds": 60
    },
    "outbox_drain": {
      "argv": ["python", "scripts/probe-postgresql-source-state.py", "--gate", "outbox_drain"],
      "timeout_seconds": 60
    }
  }
}
```

Add the other six gate commands to the same `probes` object. For AKS those
commands should be reviewed scripts that use `kubectl` or the authenticated
Studio/API probe surface and print only the exact observation object. Do not put
tokens, passwords, connection strings, or inline shell programs in the
manifest. Workload identity and the existing Kubernetes context supply access.

The checked-in live cutover probe supplies the `shadow_parity` and
`write_fence` observations. Shadow parity loads the 16 freshly reconciled,
integrity-bound family reports, rechecks counts and every required comparison,
and emits only aggregate identities. The fence probe requires the live
PostgreSQL fence configuration, executes a read probe, then attempts a zero-row
mutation against one authority table per family and passes only when the
metadata boundary rejects every attempt with its fence exception:

```powershell
python scripts/probe-kaveondb-cutover.py shadow-parity `
  --reports tmp/reconciliation-reports --max-age-hours 24

python scripts/probe-kaveondb-cutover.py write-fence `
  --deployment-revision api@IMAGE_DIGEST
```

Restart and rollback observations use reviewed command manifests whose entries
contain only `argv` and `timeout_seconds`; shell interpreters and inline command
strings are rejected. `probe-kaveondb-recovery.py restart-recovery` records
content-free state inventories before and after, ready API/Studio pod lists with
distinct UIDs, a PostgreSQL-unavailable probe, the restart command, and service
probes. `rollback` additionally requires a separate bounded control containing
the exact cutover revision, expected state digest, maximum operations, and
maximum duration. It accepts success only after target fencing and source
read/write restoration are observed:

```powershell
python scripts/probe-kaveondb-recovery.py restart-recovery `
  --manifest tmp/restart-recovery-commands.json

python scripts/probe-kaveondb-recovery.py rollback `
  --manifest tmp/rollback-commands.json `
  --control tmp/rollback-control.json
```

```powershell
python scripts/record-postgresql-operational-evidence.py `
  --manifest tmp/retirement-probe-manifest.json `
  --output tmp/retirement-observations/run-20260914 `
  --max-rollback-seconds 900
```

After the operator has recorded and hashed all eight live observations, assemble
the inputs for the existing 16-family retirement runner:

```powershell
python scripts/collect-postgresql-operational-evidence.py `
  --observations tmp/retirement-observations/run-20260914 `
  --gates tmp/reconciliation-reports/retirement-gates.json `
  --operational tmp/postgresql-operational-rehearsals.json `
  --max-age-hours 24 `
  --max-rollback-seconds 900

$env:KAVEON_RETIREMENT_EVIDENCE_COLLECTION_ENABLED = "true"
python scripts/run-postgresql-retirement-evidence.py `
  --reports tmp/reconciliation-reports `
  --evidence tmp/postgresql-reconciliation-evidence.json `
  --audit tmp/postgresql-retirement-audit.json `
  --max-age-hours 24

python scripts/retirement-qualification-summary.py `
  --audit tmp/postgresql-retirement-audit.json `
  --operational tmp/postgresql-operational-rehearsals.json `
  --output tmp/postgresql-retirement-summary.json
```

Do not scale PostgreSQL to zero unless the collector, retirement audit, and
qualification summary all exit successfully from the same fresh rehearsal
window. Retain the PostgreSQL snapshot and PVC through the rollback window.
