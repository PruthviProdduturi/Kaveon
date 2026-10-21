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

The run used two Kaveon workers and compared both engines against DuckDB results generated from the same Parquet files. The binary was built from commit `237eec03b7d839edf3787036e305e8008f554181`.

## Result

All **12/12** cases passed exact result comparison. Distributed cases reported their expected stage telemetry. `SELECT COUNT(*) FROM events` reported `execution_mode=coordinator` and zero stages because the exact count is answered by the metadata fast path; the harness now allows that mode only for this named count case and continues to reject other silent local fallbacks.

Median elapsed milliseconds (Kaveon / Trino):

| Case | Kaveon | Trino |
|---|---:|---:|
| filtered sum | 83 | 89 |
| grouped sum | 119 | 115 |
| join | 242 | 141 |
| small left join | 195 | 121 |
| TopN | 86 | 81 |
| distinct | 105 | 77 |
| metadata count | 60 | 30 |
| arithmetic projection | 519 | 56 |
| medium groups | 171 | 91 |
| high groups | 2,616 | 197 |
| multi-aggregate | 95 | 70 |
| grouped join | 209 | 123 |

The result is strong correctness evidence and a useful optimization baseline. It also shows clear performance gaps in arithmetic projection and high-cardinality grouping that remain open before claiming parity or superiority.

