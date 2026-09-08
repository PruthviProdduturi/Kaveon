#!/usr/bin/env python3
"""Generate synthetic, reproducible AKS smoke-test data; requires pyarrow==25.0.1."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
from collections import defaultdict
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq


def generate(output: Path, count: int) -> dict:
    output.mkdir(parents=True, exist_ok=True)
    customers = [
        {"customer_id": i, "customer_name": f"Synthetic customer {i:03d}",
         "region": ["east", "north", "south", "west"][(i - 1) % 4]}
        for i in range(1, 101)
    ]
    orders = [
        {"order_id": i, "customer_id": None if i % 97 == 0 else (i - 1) % 100 + 1,
         "order_date": f"2026-09-{(i - 1) % 7 + 1:02d}",
         "amount_cents": None if i % 89 == 0 else (i * 137) % 100000,
         "status": "cancelled" if i % 11 == 0 else "completed"}
        for i in range(1, count + 1)
    ]
    for name, rows in (("customers", customers), ("orders", orders)):
        raw = output / "bronze" / name
        raw.mkdir(parents=True, exist_ok=True)
        with (raw / "part-00000.csv").open("w", newline="", encoding="utf-8") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator="\n")
            writer.writeheader()
            writer.writerows(rows)
        with (raw / "part-00000.jsonl").open("w", encoding="utf-8", newline="\n") as stream:
            for row in rows:
                stream.write(json.dumps(row, sort_keys=True, separators=(",", ":")) + "\n")

    order_schema = pa.schema([
        ("order_id", pa.int64()), ("customer_id", pa.int64()),
        ("order_date", pa.string()), ("amount_cents", pa.int64()), ("status", pa.string()),
    ])
    customer_schema = pa.schema([
        ("customer_id", pa.int64()), ("customer_name", pa.string()), ("region", pa.string()),
    ])
    daily = defaultdict(list)
    for row in orders:
        if row["status"] == "completed":
            daily[row["order_date"]].append(row)
    daily_sales = [
        {"order_date": day, "order_count": len(rows),
         "amount_cents": sum(r["amount_cents"] for r in rows if r["amount_cents"] is not None)
         if any(r["amount_cents"] is not None for r in rows) else None}
        for day, rows in sorted(daily.items())
    ]
    gold_schema = pa.schema([
        ("order_date", pa.string()), ("order_count", pa.int64()), ("amount_cents", pa.int64()),
    ])
    tables = []
    for layer, name, rows, schema in (
        ("silver", "orders", orders, order_schema),
        ("silver", "customers", customers, customer_schema),
        ("gold", "daily_sales", daily_sales, gold_schema),
    ):
        path = output / layer / name / "part-00000.parquet"
        path.parent.mkdir(parents=True, exist_ok=True)
        table = pa.Table.from_pylist(rows, schema=schema)
        pq.write_table(table, path, compression="snappy", row_group_size=2048, version="2.6")
        assert pq.read_table(path).equals(table), f"Parquet roundtrip failed: {name}"
        tables.append({"name": name, "layer": layer, "path": path.relative_to(output).as_posix(),
                       "rows": len(rows), "schema": str(schema)})

    amounts = [r["amount_cents"] for r in orders if r["amount_cents"] is not None]
    region_rows = defaultdict(list)
    for row in orders:
        if row["customer_id"] is not None:
            region_rows[customers[row["customer_id"] - 1]["region"]].append(row)
    expectations = [
        {"name": "order_totals", "sql": "SELECT COUNT(*), COUNT(amount_cents), SUM(amount_cents), MIN(amount_cents), MAX(amount_cents) FROM orders",
         "rows": [[count, len(amounts), sum(amounts) if amounts else None, min(amounts) if amounts else None, max(amounts) if amounts else None]]},
        {"name": "null_customers", "sql": "SELECT COUNT(*) FROM orders WHERE customer_id IS NULL",
         "rows": [[sum(r["customer_id"] is None for r in orders)]]},
        {"name": "customer_count", "sql": "SELECT COUNT(*) FROM customers", "rows": [[100]]},
        {"name": "region_join", "sql": "SELECT c.region, COUNT(*), SUM(o.amount_cents) FROM orders o JOIN customers c ON o.customer_id = c.customer_id GROUP BY c.region ORDER BY c.region",
         "rows": [[region, len(rows), sum(r["amount_cents"] for r in rows if r["amount_cents"] is not None)
                   if any(r["amount_cents"] is not None for r in rows) else None]
                  for region, rows in sorted(region_rows.items())]},
        {"name": "silver_daily", "sql": "SELECT order_date, COUNT(*), SUM(amount_cents) FROM orders WHERE status = 'completed' GROUP BY order_date ORDER BY order_date",
         "rows": [[r["order_date"], r["order_count"], r["amount_cents"]] for r in daily_sales]},
        {"name": "gold_daily", "sql": "SELECT order_date, order_count, amount_cents FROM daily_sales ORDER BY order_date",
         "rows": [[r["order_date"], r["order_count"], r["amount_cents"]] for r in daily_sales]},
    ]
    (output / "expected-results.json").write_text(json.dumps(expectations, indent=2) + "\n", encoding="utf-8")
    files = []
    for path in sorted(output.rglob("*")):
        if path.is_file() and path.name != "manifest.json":
            files.append({"path": path.relative_to(output).as_posix(), "bytes": path.stat().st_size,
                          "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    manifest = {"fixture": "kaveon-aks-medallion-v1", "synthetic_only": True,
                "generator": "scripts/generate-medallion-fixture.py", "pyarrow_version": pa.__version__,
                "order_count": count, "tables": tables, "files": files}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("tmp/aks-medallion"))
    parser.add_argument("--orders", type=int, default=10000)
    args = parser.parse_args()
    if args.orders < 1:
        parser.error("--orders must be positive")
    result = generate(args.output, args.orders)
    print(json.dumps({"output": str(args.output.resolve()), "orders": args.orders,
                      "files": len(result["files"]), "bytes": sum(f["bytes"] for f in result["files"])}))
