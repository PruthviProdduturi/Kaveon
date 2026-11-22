# Docker exchange-loss recovery — 2026-11-22

The local release Engine binary was exercised with a three-process coordinator
and worker topology. A join query was held at the stage-2 consumer barrier;
the selected consumer worker was terminated after two producer exchange chunks
had been materialized. The coordinator retried the work and returned the exact
reference result.

| Check | Result |
|---|---|
| Input | 250,000 event rows joined to 10,000 customer rows |
| Expected result | `COUNT = 250000`, `SUM = 124875000` |
| Returned result | Exact match |
| HTTP status | 200 |
| Failed worker | worker 1 |
| Exchange chunks before failure | 2 |
| Exchange chunks after completion | 0 |
| Engine binary SHA-256 | `5627b9f265b3d40246d77e607c68174d294fb19018b38d2dcc79c98eecc3d338` |

Command:

```powershell
python engine/qualification/exchange_loss.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --rows 250000 `
  --output tmp/qualification-exchange-loss-2026-11-22
```

This closes the local exchange-loss/retry cleanup check for this image and
fixture. It does not close cloud object-store fault injection, node loss,
rolling upgrade, or multi-coordinator recovery gates.
