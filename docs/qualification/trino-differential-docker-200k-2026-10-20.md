# Trino differential qualification — Docker, 200k rows

Status: **passed for correctness; performance gap remains**

The local qualification image was compared with Trino on the same generated
Parquet files using the harness-supported Docker topology:

```powershell
python engine/qualification/same_files.py `
  --docker-image kaveon-engine:qualification-local `
  --rows 200000 `
  --customers 20000 `
  --suite extended `
  --warmups 1 `
  --repetitions 1 `
  --workers 0 `
  --output tmp/same-files-docker-200k-rerun
```

All **12/12** cases returned exact matching results. The report marks the
comparison fair, but the run is single-node and Kaveon is slower on this
sample; it is not evidence of a performance win. The publication workload gate
was not enabled. The single-node run remains the performance baseline; the
newly supported two-worker correctness run is recorded separately in
`trino-differential-docker-2worker-200k-2026-10-20.md`.

