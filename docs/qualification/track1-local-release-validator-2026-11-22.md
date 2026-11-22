# Local release evidence validator — 2026-11-22

The local Engine evidence is now machine-checked rather than accepted from
individual prose receipts. The validator requires the distributed exact-result,
exchange-loss, pressure/cleanup, and soak reports plus the exact binary used
to produce them. It checks the binary digest and fails closed when any report
is missing, malformed, or does not meet its invariants.

```powershell
python scripts/verify-engine-local-release.py `
  --docker-report tmp/qualification-docker-2worker-current/report.json `
  --exchange-report tmp/qualification-exchange-loss-2026-11-22/report.json `
  --pressure-report tmp/qualification-pressure-2026-11-22/report.json `
  --soak-report tmp/qualification-soak-2026-11-22/report.json `
  --binary engine/target/release/kaveon-server.exe `
  --output tmp/qualification-local-release-gate-2026-11-22.json
```

The output scope is `local-docker`; it deliberately reports cloud gates as
`pending` and cannot be used to claim ADLS or AKS production qualification.
