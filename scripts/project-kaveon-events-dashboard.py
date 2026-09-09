"""Build the compact synthetic event table used by the canonical dashboards.

The input is ``kaveon_product_analytics``.  Each existing user-day remains one
row and is assigned to one of six product surfaces with a stable hash.  This
keeps the projection compact while supplying the complete 19-column contract
used by the live product charts.  Derived values are showcase telemetry; they
are not represented as an export of the legacy 504M-row PostgreSQL view.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import date, datetime
from pathlib import Path
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq


SURFACES = ("Chat", "Dashboard", "Chart Builder", "SQL Lab", "API", "Export")
REQUIRED_INPUT = {
    "usage_date", "user_id", "queries_run", "nl_queries", "sql_lab_runs",
    "dashboards_viewed", "charts_created", "exports", "api_calls",
    "data_processed_mb", "errors", "sessions", "license", "segment",
    "industry", "team_size", "deployment", "acquisition_channel", "country",
    "region", "platform",
}
OUTPUT_SCHEMA = pa.schema([
    pa.field("user_id", pa.int64()),
    pa.field("surface", pa.string()),
    pa.field("actions", pa.int64()),
    pa.field("sessions", pa.int64()),
    pa.field("queries_run", pa.int64()),
    pa.field("charts_created", pa.int64()),
    pa.field("errors", pa.int64()),
    pa.field("rows_scanned", pa.int64()),
    pa.field("cache_hits", pa.int64()),
    pa.field("latency_p75_ms", pa.int64()),
    pa.field("platform", pa.string()),
    pa.field("license", pa.string()),
    pa.field("segment", pa.string()),
    pa.field("industry", pa.string()),
    pa.field("region", pa.string()),
    pa.field("country", pa.string()),
    pa.field("deployment", pa.string()),
    pa.field("acquisition_channel", pa.string()),
    pa.field("team_size", pa.string()),
])


def day_number(value: Any) -> int:
    if isinstance(value, datetime):
        return value.date().toordinal()
    if isinstance(value, date):
        return value.toordinal()
    return date.fromisoformat(str(value)[:10]).toordinal()


def integer(value: Any) -> int:
    return 0 if value is None else int(value)


def project(row: dict[str, Any]) -> dict[str, Any]:
    user_id = integer(row["user_id"])
    day = day_number(row["usage_date"])
    queries = max(0, integer(row["queries_run"]))
    errors = max(0, integer(row["errors"]))
    actions = sum(max(0, integer(row[name])) for name in (
        "nl_queries", "sql_lab_runs", "dashboards_viewed", "charts_created",
        "exports", "api_calls",
    ))
    processed_mb = max(0.0, float(row["data_processed_mb"] or 0.0))
    successful_queries = max(0, queries - errors)
    return {
        "user_id": user_id,
        "surface": SURFACES[(user_id * 31 + day * 17) % len(SURFACES)],
        "actions": actions,
        "sessions": max(0, integer(row["sessions"])),
        "queries_run": queries,
        "charts_created": max(0, integer(row["charts_created"])),
        "errors": errors,
        "rows_scanned": round(processed_mb * 1024),
        "cache_hits": round(successful_queries * (25 + (user_id + day) % 51) / 100),
        "latency_p75_ms": 50 + (user_id * 97 + day * 13 + queries * 31) % 2951,
        **{name: None if row[name] is None else str(row[name]) for name in (
            "platform", "license", "segment", "industry", "region", "country",
            "deployment", "acquisition_channel", "team_size",
        )},
    }


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def materialize(source: Path, output: Path, batch_rows: int = 25_000) -> dict[str, Any]:
    parquet = pq.ParquetFile(source)
    missing = REQUIRED_INPUT - set(parquet.schema_arrow.names)
    if missing:
        raise RuntimeError(f"input is missing columns: {', '.join(sorted(missing))}")
    destination = output / "public" / "kaveon_events_dashboard" / "part-00000.parquet"
    destination.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    with pq.ParquetWriter(destination, OUTPUT_SCHEMA, compression="zstd") as writer:
        for batch in parquet.iter_batches(batch_size=batch_rows, columns=sorted(REQUIRED_INPUT)):
            projected = [project(row) for row in batch.to_pylist()]
            writer.write_table(pa.Table.from_pylist(projected, schema=OUTPUT_SCHEMA))
            count += len(projected)
    table = {
        "schema": "public",
        "name": "kaveon_events_dashboard",
        "location": "public/kaveon_events_dashboard/part-00000.parquet",
        "row_count": count,
        "parquet_bytes": destination.stat().st_size,
        "sha256": sha256(destination),
        "columns": [
            {"name": field.name, "data_type": "Utf8" if pa.types.is_string(field.type) else "Int64", "nullable": True}
            for field in OUTPUT_SCHEMA
        ],
        "lineage": {
            "kind": "synthetic_showcase_projection",
            "source": "kaveon_product.kaveon_product_analytics",
            "grain": "one deterministic surface per user-day",
            "surface_assignment": "(user_id * 31 + ordinal(usage_date) * 17) modulo 6",
            "derived_columns": ["surface", "actions", "rows_scanned", "cache_hits", "latency_p75_ms"],
            "legacy_equivalent": False,
        },
    }
    manifest = {"format": "kaveon.events-dashboard-projection/v1", "tables": [table]}
    (output / "kaveon-events-dashboard-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True, help="combined kaveon_product_analytics Parquet")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--batch-rows", type=int, default=25_000)
    args = parser.parse_args()
    if args.batch_rows < 1:
        parser.error("--batch-rows must be positive")
    manifest = materialize(args.input, args.output, args.batch_rows)
    table = manifest["tables"][0]
    print(f"Projected {table['row_count']} synthetic event rows to {table['location']}")


if __name__ == "__main__":
    main()
