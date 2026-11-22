# Local pressure and admission qualification — 2026-11-22

## Result

**PASS.** The constrained two-worker pressure harness passed **11/11 cases**
with 100,000 fixture rows and a 256 MiB query pool. Successful grouped,
join, sort, Top-N, set-operation, bounded-window, and skew-join cases matched
the reference results; window/repeat/disk-quota refusal cases failed closed;
and a running-window cancellation recovered cleanly.

Command:

```powershell
& engine/qualification/venv/Scripts/python.exe engine/qualification/pressure.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --rows 100000 --workers 2 --memory-mib 256 `
  --output tmp/qualification-pressure-local-20261122
```

The complete report is retained at
`tmp/qualification-pressure-local-20261122/report.json`. No cloud resources
were provisioned or accessed; incremental cloud spend was **$0**.

## Boundary

This closes the local admission/pressure contract. It does not replace cloud
disk-exhaustion, node-pressure, or long-duration AKS qualification.
