# Docker pressure, spill, and cancellation — 2026-11-22

The current release binary passed the bounded two-worker pressure suite on a
200,000-row fixture. The suite checks correctness, resource refusals, external
sort spill, cancellation, and cleanup rather than treating process RSS as the
logical query-pool limit.

| Gate | Result |
|---|---|
| Grouped aggregate, join, TopN, set operation | Exact result, passed |
| External sort spill | Passed; spill observed and cleaned |
| Bounded window | Exact result, passed |
| Skewed join | Exact result, passed |
| Window memory refusal | Failed closed at the configured 32 MiB limit |
| Oversized string expression | Failed closed at the batch-output bound |
| Spill disk quota | Failed closed at the 1 KiB quota |
| Running-window cancellation | HTTP cancel 204; terminal query cancellation observed; admission recovered in 78 ms |
| Overall report | `passed: true` |

Command:

```powershell
python engine/qualification/pressure.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --rows 200000 --workers 2 --memory-mib 32 `
  --output tmp/qualification-pressure-2026-11-22
```

Binary SHA-256:
`5627b9f265b3d40246d77e607c68174d294fb19018b38d2dcc79c98eecc3d338`.

This closes the local bounded-pressure, spill, and cancellation check for the
image. It does not claim cloud endurance, object-store fault injection, or
multi-coordinator recovery.
