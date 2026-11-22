# Local differential smoke qualification — 2026-11-22

## Result

**PASS.** The release server binary was checked against DuckDB and Trino 483
using the pinned local qualification fixtures:

```text
python engine/qualification/smoke.py \
  --server-bin engine/target/release/kaveon-server.exe \
  --regressions \
  --output tmp/qualification-local-release-20261122
```

All **37/37** cases passed with exact result parity, including scans,
filters, grouped aggregates, joins, Top-N, NULL-sensitive `IN`/`EXISTS`,
windows, set operations, decimal arithmetic, and large-integer aggregates.
Paging and the security checks also passed. The report records commit
`ce908e5d`, the release binary SHA-256, fixture hashes, and reference versions
(DuckDB 1.5.5, PyArrow 25.0.1, Trino 483) at
`tmp/qualification-local-release-20261122/report.json`.

The reference Trino/PostgreSQL services were pinned local containers only;
no cloud resources were provisioned and incremental cloud spend was **$0**.
This is a deterministic correctness gate, not a broad performance or cloud
production claim.
