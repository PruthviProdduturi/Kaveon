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
| `restart_recovery.json` | PostgreSQL unavailable, new API and Studio processes ready, nonzero probes, and identical state digests before/after |
| `rollback.json` | Cutover revision, target fenced, source reads/writes restored within the recovery bound, and identical state digests |
| `backup_identity.json` | Backup identity/digest, restore job, positive restored table count, and identical source/restored inventory digests |
| `durable_checkpoint.json` | Different pod UIDs, identical checkpoint digest, non-regressing position, and completed resume |

Each JSON object has exactly these top-level fields:

```json
{
  "schema_version": 1,
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
