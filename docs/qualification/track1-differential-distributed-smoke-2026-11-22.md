# Distributed differential smoke qualification — 2026-11-22

## Result

**PASS for the local two-worker release server.** The distributed smoke gate
completed with `workers=2` and `passed=true`. Five distributed SQL cases had
exact result parity against the pinned local references: scan, filtered count,
grouped count, join count, and Top-N. All five reported distributed stages
(2, 2, 3, 4, and 2 respectively). The harness also passed its paging check.

Command:

```powershell
python engine/qualification/smoke.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --workers 2 `
  --output tmp/qualification-distributed-release-20261122
```

The machine-readable report is retained at
`tmp/qualification-distributed-release-20261122/report.json` and records the
release binary, commit, fixture hashes, and reference versions.

This qualifies the local two-worker correctness path only; it is not AKS
failure, cloud identity, multi-coordinator, or performance evidence. No cloud
resources were provisioned and incremental cloud spend was **$0**.
