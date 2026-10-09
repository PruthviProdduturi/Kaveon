# Distributed operational soak qualification — 2026-10-09

The native two-worker soak ran the release server binary against disposable
Parquet fixtures for 60 seconds. It intentionally exercised cancellation,
queueing, worker loss, paged results, query-history cleanup and resource
retention. The fixture uses explicit admin principals because it has no
KaveonDB authority from which to import catalog grants; role isolation is
covered separately by the distributed smoke qualification.

## Result

- **306 queries** completed with exact DuckDB checksums.
- **2 cancellation/queue controls** passed.
- A worker was intentionally terminated and the surviving cluster completed
  the recovery probe.
- Workload duration: **61.562 seconds**.
- Median latency: **172 ms**; p95: **375 ms**.
- Query history stayed bounded, retained files were fully cleaned, the monitor
  stayed responsive, RSS growth and peak limits passed, and surviving workers
  remained healthy.
- Overall qualification: **PASS**.

Command:

```powershell
python engine/qualification/soak.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --output tmp-soak-local-v3.json `
  --duration-seconds 60 --workers 2 --rows 100000 `
  --max-rss-growth-mib 512
```

This is local operational evidence. AKS/ADLS rolling-restart, cloud object
store failure and multi-day production soak evidence remain separate gates.
