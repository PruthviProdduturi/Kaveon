# Local mixed-workload soak qualification — 2026-11-22

## Result

**PASS.** The bounded native two-worker soak ran for **120.11 seconds** with
**941 queries**, five control cycles, and an intentional worker loss. All
queries returned exact expected results and every operational check passed:

- cancellation and queue controls remained responsive;
- worker-loss recovery was exercised and surviving nodes stayed alive;
- query history stayed bounded;
- retained exchange/spill files were cleaned up;
- monitor responsiveness, RSS growth, and RSS peak stayed within limits.

Observed latency was 156 ms median and 328 ms p95 for this fixture. This is
operability evidence, not a broad performance claim.

Command:

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/soak.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --duration-seconds 120 --workers 2 --rows 100000 `
  --output tmp/qualification-soak-local-20261122
```

The complete report is retained at
`tmp/qualification-soak-local-20261122/report.json`. No cloud resources were
provisioned or accessed; incremental cloud spend was **$0**.

## Boundary

This closes the local bounded-soak gate only. It does not replace the longer
AKS/ADLS soak, node-drain, rolling-upgrade, backup/restore, or multi-coordinator
qualification gates.
