# Track 1 local operational harness — 2026-10-17

The repository-level operational harness for upgrade, rollback, restart
recovery, ADLS restore, and retirement-evidence validation is implemented and
passes its focused test suite.

## Evidence

From the repository root:

```powershell
python -m pytest `
  scripts/test_audit_postgresql_operational_receipts.py `
  scripts/test_probe_aks_postgresql_retirement.py `
  scripts/test_probe_kaveondb_recovery.py `
  scripts/test_record_postgresql_operational_evidence.py `
  scripts/test_rehearse_kaveondb_adls_restore.py `
  scripts/test_retirement_evidence_helm.py `
  scripts/test_retirement_qualification_summary.py -q
```

Result on 2026-10-17: **26 passed, 0 failed**.

The tests cover machine-readable failure behavior, restore cleanup publication
only after a successful copy, restart and rollback probe dispatch, receipt
age/rollback-bound validation, Helm retirement evidence checks, and the
fail-closed final qualification summary.

## Boundary

This is local implementation evidence for the operational harness. It does not
qualify a live AKS upgrade, PostgreSQL-unavailable restart, ADLS backup/restore,
rollback window, or sustained mixed-workload soak. Those still require one
immutable live run with archived receipts and the complete 16-family retirement
report described in `docs/engineering/aks-postgresql-retirement-rehearsal.md`.
