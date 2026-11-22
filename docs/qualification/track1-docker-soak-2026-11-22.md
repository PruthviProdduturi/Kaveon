# Docker endurance soak — 2026-11-22

The native Engine release binary completed a 45-second requested soak on a
two-worker topology. The harness reached 50.25 seconds of actual workload,
executed 231 queries and two control operations, intentionally killed one
worker, and checked every result and cleanup invariant.

| Check | Result |
|---|---|
| Query results | All exact results passed |
| Worker loss | Exercised; surviving nodes remained alive |
| Cancellation and queue recovery | Passed |
| History bound | Passed |
| Retained files | Cleaned |
| Monitor responsiveness | Passed |
| RSS growth/peak bounds | Passed |
| Latency | Median 172 ms; p95 359 ms |
| Overall | `passed: true` |

Command:

```powershell
python engine/qualification/soak.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --duration-seconds 45 --workers 2 --rows 100000 `
  --output tmp/qualification-soak-2026-11-22
```

This is local endurance evidence for the current image. It does not replace
the cloud ADLS, rolling-upgrade, multi-coordinator, or object-store fault
qualification gates.
