# Trino differential qualification — release rerun, 500k rows

Status: **passed for correctness**

The current release Engine binary was compared with Trino on the same generated
Parquet files:

```powershell
python engine/qualification/same_files.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --rows 500000 `
  --customers 50000 `
  --suite extended `
  --warmups 1 `
  --repetitions 2 `
  --workers 2 `
  --output tmp/same-files-release-500k-rerun
```

All **12/12** cases returned exact matching rows:

`filtered_sum`, `grouped_sum`, `join`, `small_left_join`, `topn`, `distinct`,
`unfiltered_count`, `arithmetic_projection`, `medium_groups`, `high_groups`,
`multi_aggregate`, and `grouped_join`.

The report is `tmp/same-files-release-500k-rerun/report.json` and records
`passed: true`. This run is a correctness qualification only: the harness did
not mark the fair-performance comparison as qualified, so these results do not
support a claim that Kaveon is faster than Trino.

