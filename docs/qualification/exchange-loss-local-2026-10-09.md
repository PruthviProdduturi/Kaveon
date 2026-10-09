# Distributed exchange-loss qualification — 2026-10-09

The corrected local fault-injection harness (`engine/qualification/exchange_loss.py`)
ran against the release server binary with its default **1,000,000-row** fixture.
It proxies both workers, waits until an actual stage-2 consumer has received the
upstream exchange, stops that worker, then releases the request so the
coordinator must recover the consumer stage from the durable exchange spool.

## Result

| Check | Result |
|---|---:|
| Producer exchange chunks before failure | 2 |
| Consumer worker terminated | worker 1 |
| HTTP result | 200 |
| Returned rows | `1,000,000` |
| Returned sum | `499,500,000` |
| DuckDB expected rows | `1,000,000` |
| DuckDB expected sum | `499,500,000` |
| Exchange files after completion | 0 |
| Qualification | **PASS** |

Command:

```powershell
python engine/qualification/exchange_loss.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --output tmp-exchange-loss-1m.json
```

Binary SHA-256:
`0ef4d76c806b3e27b9c6c15d75266fb4fb32a6c6aa0d285012d9bde36477f2f5`

This is local runtime evidence. It qualifies exchange recovery for the tested
fixture; it does not replace AKS/ADLS qualification, sustained concurrency,
cloud object-store failure testing, or a long-duration soak.
