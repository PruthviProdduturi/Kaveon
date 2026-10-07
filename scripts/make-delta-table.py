"""Write a Delta table beside its Parquet source, for the showcase.

The lake's published datasets are plain Parquet, which is right for data
nobody writes — a transaction log over read-only reference data is one more
indirection before every read, and earns nothing. But Kaveon claims Delta and
Iceberg as first-class next to Parquet, and a showcase made entirely of
Parquet demonstrates one third of that claim.

This writes one table in Delta so the path is shown, not only asserted: same
rows, a real `_delta_log`, read by the Engine's own Delta reader rather than
its Parquet reader.
"""
import sys
from pathlib import Path

import pyarrow.parquet as pq
from deltalake import write_deltalake

source = Path(sys.argv[1])
target = Path(sys.argv[2])

table = pq.read_table(source)
print(f"read {table.num_rows:,} rows, {table.num_columns} columns from {source.name}")

if target.exists():
    sys.exit(f"{target} exists; remove it to rewrite")
write_deltalake(str(target), table, mode="error")

log = sorted((target / "_delta_log").glob("*.json"))
parts = sorted(p for p in target.glob("*.parquet"))
print(f"wrote {target}")
print(f"  _delta_log: {[p.name for p in log]}")
print(f"  data files: {len(parts)}")
