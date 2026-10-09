# Distributed semantic smoke qualification — 2026-10-09

The local qualification stack used the release server binary and the pinned
Trino 483 reference container from `engine/qualification/compose.yml`. Kaveon
ran with two workers and four concurrent submissions.

## Result

- **37/37** semantic cases passed against DuckDB and Trino, including scans,
  filters, grouped aggregates, joins, TopN, windows, subqueries, set
  operations, NULL behavior, decimals and large integer aggregates.
- Paged results passed: **2,501 rows over 3 pages**, with replay stability.
- Concurrency passed: **12/12** exact-result requests admitted and completed.
- Worker-loss case passed: the query completed with the expected result after
  a worker was removed.
- Security checks passed: anonymous statements denied, reader execution denied,
  and a public token could not dispatch an internal task.

Command:

```powershell
docker compose -f engine/qualification/compose.yml up -d trino
python engine/qualification/smoke.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --workers 2 --concurrency 4 --worker-loss --regressions `
  --output tmp-smoke-trino-v2.json
```

This is semantic and recovery evidence, not a performance claim. AKS/ADLS
qualification, sustained soak, rolling restart, and cloud object-store failure
evidence remain open.
