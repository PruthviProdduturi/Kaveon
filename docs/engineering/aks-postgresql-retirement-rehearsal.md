# AKS PostgreSQL retirement rehearsal

This runbook executes evidence collection; it does not declare PostgreSQL safe
to delete. Keep PostgreSQL, its PVC, and its snapshot throughout the rollback
window. Use a new immutable run ID and backup/restore prefix for every attempt.

Prepare a private evidence PVC, the live reconciliation manifest/checkpoints,
the DLM and context-cache observations, and a Helm overlay containing resource
names, workload-identity client IDs, Key Vault/ADLS URLs, image digests, and
cutover booleans only. Never put credentials, tokens, connection strings, SAS
URLs, or account keys in the overlay. Start with retirement and restart modes
false. Validate locally:

```powershell
helm lint kaveon infra/helm/kaveon-portal-test -f $values
helm template kaveon infra/helm/kaveon-portal-test -f $values > tmp/retirement-rendered.yaml
```

Create a JSON plan with `schema_version: 1`, one immutable `run_id`, and these
steps in this exact order. Every step contains only `id` and a nonempty
`commands` array; each command contains `argv` and `timeout_seconds` (1–3600).
Use absolute local paths. Do not use a shell, `python -c`, or `python -m`.

1. `snapshot_inventory`: run the reviewed source-state and live-inventory
   scripts, persisting the source watermark and discovered authority tables.
2. `helm_migration`: `helm upgrade --install kaveon
   infra/helm/kaveon-portal-test -n kaveon -f <absolute-overlay> --atomic
   --wait`; enable outbox/replay, DLM migration, and the two special report jobs.
3. `dlm_migration`: wait for the revision-named DLM migration and both special
   report Jobs, then copy their restricted evidence from the PVC.
4. `reconcile_16`: run `postgresql_reconciliation_report_cli.py` against the
   complete manifest and require exactly 16 reports.
5. `shadow_parity`: run `probe-kaveondb-cutover.py shadow-parity --reports
   <reports> --max-age-hours 1`.
6. `fence_drain`: deploy the overlay with every read authority family, the
   PostgreSQL fence, and replay enabled; run the fence probe and source-state
   drain probe until the fixed watermark has zero pending events.
7. `backup`: run `create-kaveondb-adls-backup.py` with the active product
   transaction prefix and a new backup ID.
8. `restore`: run `rehearse-kaveondb-adls-restore.py restore` into a new restore
   prefix. Archive its observation and ETag-bound cleanup manifest.
9. `restart_recovery`: deploy `restartRehearsalMode: true` only after the
   preliminary audit is mounted, then run `probe-kaveondb-recovery.py
   restart-recovery` with reviewed kubectl commands. The `service_probes`
   command must emit the complete fresh report from
   `postgresql_free_smoke_cli.py`; a success flag or probe count is rejected.
10. `rollback`: run `probe-kaveondb-recovery.py rollback` with the exact
    cutover revision, pre-cutover state digest, maximum 10,000 operations, and a
    recovery-time bound no longer than 900 seconds.
11. `collect_operational`: run `record-postgresql-operational-evidence.py` and
    `collect-postgresql-operational-evidence.py` for the same run window.
12. `final_audit`: run `run-postgresql-retirement-evidence.py`, followed by
    `retirement-qualification-summary.py`. A nonzero exit, missing output, old
    evidence, or anything other than 16 passing families stops retirement.

Execute or resume the immutable plan:

```powershell
python scripts/run-aks-postgresql-retirement.py `
  --plan tmp/retirement-plan.json `
  --values $values `
  --checkpoint tmp/retirement-plan.checkpoint.json
```

The checkpoint contains only the run ID, plan digest, and completed prefix of
steps. A modified plan or reordered/skipped step cannot resume. The runner
records a step only after exit code zero and stops on timeout or failure. Store
all receipts on the restricted evidence volume and copy the final audit and
qualification summary to the operations archive before any scaling decision.

After review, cleanup only the rehearsal restore prefix with its generated
cleanup manifest. Retain the immutable backup, PostgreSQL snapshot, and PVC for
the approved rollback period. Scaling PostgreSQL to zero is a separate operator
action after the final audit; deletion is outside this rehearsal.
