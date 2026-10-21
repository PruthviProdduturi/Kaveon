# Trino differential qualification — release build, 500k rows

This run compares exact results and observed latency for the extended fixed-file corpus. It is a correctness and diagnostic qualification, not a claim that Kaveon is faster than Trino.

## Configuration

```powershell
python engine/qualification/same_files.py `
  --server-bin engine/target/release/kaveon-server.exe `
  --rows 500000 --customers 50000 `
  --repetitions 2 --warmups 1 --suite extended --workers 2 `
  --output tmp/same-files-release-500k-final
```

The run used two Kaveon workers and compared both engines against DuckDB results generated from the same Parquet files. The rerun below uses the direct primitive JSON-number encoding optimization; its commit is recorded with the evidence update.

## Result

All **12/12** cases passed exact result comparison. Distributed cases reported their expected stage telemetry. `SELECT COUNT(*) FROM events` reported `execution_mode=coordinator` and zero stages because the exact count is answered by the metadata fast path; the harness now allows that mode only for this named count case and continues to reject other silent local fallbacks.

Median elapsed milliseconds (Kaveon / Trino):

| Case | Kaveon | Trino |
|---|---:|---:|
| filtered sum | 74 | 68 |
| grouped sum | 93 | 67 |
| join | 188 | 121 |
| small left join | 202 | 117 |
| TopN | 84 | 89 |
| distinct | 116 | 83 |
| metadata count | 48 | 31 |
| arithmetic projection | 445 | 74 |
| medium groups | 168 | 110 |
| high groups | 2,239 | 238 |
| multi-aggregate | 109 | 81 |
| grouped join | 198 | 137 |

The result is strong correctness evidence. Direct primitive JSON-number construction reduced the arithmetic projection median from 519 ms to 445 ms and high-cardinality grouping from 2,616 ms to 2,239 ms in the comparable release rerun. The remaining gaps are still open before claiming parity or superiority.

