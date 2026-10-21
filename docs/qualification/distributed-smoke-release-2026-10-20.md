# Distributed release smoke qualification — 2026-10-20

Status: **passed**

The release Engine binary was exercised through a real coordinator with two
worker processes. The qualification command was:

```powershell
python engine/qualification/smoke.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --workers 2 `
  --output tmp/qualification-distributed-release-rerun
```

The run exited zero and passed all six cases:

- authenticated paging
- ordered scan
- filtered count
- grouped count
- distributed join count
- global TopN

Every case returned the expected exact rows, preserved the qualification
identity, and used distributed stages. The complete machine-readable report is
`tmp/qualification-distributed-release-rerun/report.json`; the temporary output
is intentionally ignored by Git. This is a bounded local release qualification,
not evidence of AKS, remote object-store, or endurance behavior.

