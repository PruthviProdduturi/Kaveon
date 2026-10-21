# Local pressure qualification — 2026-10-20

This run qualifies the current Engine build without AKS or an external object store. It is a bounded local safety and correctness gate, not a publication-scale performance claim.

## Run

```powershell
python engine/qualification/pressure.py `
  --server-bin engine/target/debug/kaveon-server.exe `
  --workers 2 --rows 200000 --memory-mib 512 `
  --output tmp/pressure-local
```

The binary was built from commit `df1400d68a9f95def18cb543fb3c21a08582d0d8` and has SHA-256 `7d6a7ed526964e606f8c94a18959f665d402ff914d5c61e0093c577da3306c67`. The run used two workers, 200,000 fact rows, a 512 MiB logical query pool, and a 256 MiB spill pool. The complete machine-readable report is `tmp/pressure-local/report.json` (generated evidence is kept out of Git).

## Result

All 11 cases passed:

- grouped aggregate, join, sort, TopN, set operation, bounded window, and skewed join completed with deterministic result hashes;
- oversized window reservation failed closed instead of exceeding the query budget;
- oversized string output failed at the 64 MiB batch guard;
- a 1 KiB spill quota failed closed and left no spill files;
- cancellation returned HTTP 204, reached the canceled terminal path, recovered admission in 78 ms, and cleaned spill state.

The run reported no retained spill files. RSS values are diagnostic process samples, not the logical operator-memory accounting; short peaks can be missed. The successful join took 44.2 seconds at this deliberately bounded local size, so this evidence must not be presented as a large-scale latency or throughput claim.

## Remaining qualification

This closes the local safety gate. The remaining work is larger-scale and deployment-specific:

1. Repeat skew, join, aggregation, and spill tests at publication scale with sustained mixed concurrency.
2. Exercise lease heartbeat/expiry and stale-attempt fencing under long pressure and more worker topologies.
3. Qualify remote ADLS/S3 split assignment, object-store retries, and cloud restart/rolling behavior.
4. Expand distributed ordered-result/window qualification and external Trino-client compatibility.
5. Continue the broader SQL/DML and ClickHouse comparison suites; this run does not establish superiority over either engine.

