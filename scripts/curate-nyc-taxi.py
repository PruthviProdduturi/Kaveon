"""Create a bounded, local January 2025 NYC Taxi bronze/silver/gold batch.

The only network reads are the fixed official TLC CloudFront files. This script
does not upload data, create cloud resources, or register Engine catalogs.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
from collections import defaultdict
from datetime import datetime
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import requests


SOURCES = {
    "yellow": "https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_2025-01.parquet",
    "green": "https://d37ci6vzurychx.cloudfront.net/trip-data/green_tripdata_2025-01.parquet",
    "zones": "https://d37ci6vzurychx.cloudfront.net/misc/taxi_zone_lookup.csv",
}
BATCH_ROWS = 65_536
ENGINE_TYPES = {"int64": "Int64", "float64": "Float64", "string": "Utf8"}


def download(url: str, destination: Path) -> dict:
    """Stream one fixed official source to disk and return immutable evidence."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    digest, size = hashlib.sha256(), 0
    with requests.get(url, stream=True, timeout=(15, 120)) as response:
        response.raise_for_status()
        with destination.open("wb") as stream:
            for chunk in response.iter_content(chunk_size=1024 * 1024):
                if chunk:
                    stream.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
    return {"url": url, "raw_sha256": digest.hexdigest(), "bytes": size}


def normalized_type(field: pa.Field) -> pa.DataType:
    kind = field.type
    if pa.types.is_integer(kind):
        return pa.int64()
    if pa.types.is_floating(kind) or pa.types.is_decimal(kind):
        return pa.float64()
    return pa.string()


def normalized_schema(schema: pa.Schema, rejection: bool = False) -> pa.Schema:
    fields = [pa.field(field.name, normalized_type(field), nullable=True) for field in schema]
    if rejection:
        fields.append(pa.field("rejection_reason", pa.string(), nullable=False))
    return pa.schema(fields)


def normalize_value(value, target: pa.DataType):
    if value is None:
        return None
    if pa.types.is_int64(target):
        return int(value)
    if pa.types.is_float64(target):
        return float(value)
    if isinstance(value, datetime):
        return value.isoformat()
    if hasattr(value, "isoformat"):
        return value.isoformat()
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def normalize_batch(batch: pa.RecordBatch, schema: pa.Schema) -> dict[str, list]:
    """Convert a RecordBatch without loading the whole source file."""
    values = {}
    for index, field in enumerate(schema):
        values[field.name] = [normalize_value(value, field.type) for value in batch.column(index).to_pylist()]
    return values


def as_finite(value) -> float | None:
    if value is None:
        return None
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def as_cents(value) -> int | None:
    if value is None:
        return None
    try:
        amount = Decimal(str(value))
        if not amount.is_finite():
            return None
        return int((amount * 100).quantize(Decimal("1"), rounding=ROUND_HALF_UP))
    except (InvalidOperation, ValueError):
        return None


def rejection_reason(pickup, dropoff, distance, amount) -> str | None:
    if not isinstance(pickup, datetime) or pickup.year != 2025 or pickup.month != 1:
        return "pickup_outside_january_2025"
    if not isinstance(dropoff, datetime) or dropoff < pickup:
        return "dropoff_before_pickup_or_missing"
    if as_finite(distance) is None or as_finite(distance) < 0:
        return "trip_distance_not_finite_nonnegative"
    if as_finite(amount) is None or as_finite(amount) < 0:
        return "total_amount_not_finite_nonnegative"
    return None


def table_spec(schema_name: str, name: str, location: str, schema: pa.Schema, row_count: int) -> dict:
    columns = []
    for field in schema:
        if pa.types.is_int64(field.type):
            kind = ENGINE_TYPES["int64"]
        elif pa.types.is_float64(field.type):
            kind = ENGINE_TYPES["float64"]
        else:
            kind = ENGINE_TYPES["string"]
        columns.append({"name": field.name, "data_type": kind, "nullable": field.nullable})
    return {"schema": schema_name, "name": name, "location": location, "columns": columns, "row_count": row_count}


def write_table(path: Path, schema: pa.Schema, columns: dict[str, list]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(pa.Table.from_pydict(columns, schema=schema), path, compression="snappy")


def curate_trips(service: str, raw_path: Path, work: Path, aggregate: dict) -> tuple[list[dict], dict]:
    file = pq.ParquetFile(raw_path)
    output_schema = normalized_schema(file.schema_arrow)
    rejected_schema = normalized_schema(file.schema_arrow, rejection=True)
    pickup_name = "tpep_pickup_datetime" if service == "yellow" else "lpep_pickup_datetime"
    dropoff_name = "tpep_dropoff_datetime" if service == "yellow" else "lpep_dropoff_datetime"
    required = {pickup_name, dropoff_name, "trip_distance", "total_amount"}
    if not required.issubset(file.schema_arrow.names):
        raise ValueError(f"{service} source does not contain required trip fields")

    bronze_location = f"bronze/{service}_trips/part-00000.parquet"
    silver_location = f"silver/{service}_trips/part-00000.parquet"
    rejected_location = f"silver/{service}_rejected/part-00000.parquet"
    paths = [work / bronze_location, work / silver_location, work / rejected_location]
    for path in paths:
        path.parent.mkdir(parents=True, exist_ok=True)
    source_count = accepted = rejected = 0
    with pq.ParquetWriter(paths[0], output_schema, compression="snappy") as bronze_writer, \
            pq.ParquetWriter(paths[1], output_schema, compression="snappy") as silver_writer, \
            pq.ParquetWriter(paths[2], rejected_schema, compression="snappy") as rejected_writer:
        for batch in file.iter_batches(batch_size=BATCH_ROWS):
            original = {name: batch.column(index).to_pylist() for index, name in enumerate(file.schema_arrow.names)}
            normalized = normalize_batch(batch, output_schema)
            bronze_writer.write_table(pa.Table.from_pydict(normalized, schema=output_schema))
            keep = {field.name: [] for field in output_schema}
            reject = {field.name: [] for field in rejected_schema}
            for row in range(batch.num_rows):
                source_count += 1
                reason = rejection_reason(original[pickup_name][row], original[dropoff_name][row],
                                          original["trip_distance"][row], original["total_amount"][row])
                target = reject if reason else keep
                for field in output_schema:
                    target[field.name].append(normalized[field.name][row])
                if reason:
                    rejected += 1
                    reject["rejection_reason"].append(reason)
                else:
                    accepted += 1
                    pickup = original[pickup_name][row]
                    distance = as_finite(original["trip_distance"][row])
                    cents = as_cents(original["total_amount"][row])
                    aggregate[(pickup.date().isoformat(), service)]["trip_count"] += 1
                    aggregate[(pickup.date().isoformat(), service)]["total_amount_cents"] += cents
                    aggregate[(pickup.date().isoformat(), service)]["total_trip_distance"] += distance
            if keep[output_schema.names[0]]:
                silver_writer.write_table(pa.Table.from_pydict(keep, schema=output_schema))
            if reject[rejected_schema.names[0]]:
                rejected_writer.write_table(pa.Table.from_pydict(reject, schema=rejected_schema))
    if accepted + rejected != source_count:
        raise RuntimeError(f"{service} reconciliation failed")
    specs = [
        table_spec("bronze", f"{service}_trips", bronze_location, output_schema, source_count),
        table_spec("silver", f"{service}_trips", silver_location, output_schema, accepted),
        table_spec("silver", f"{service}_rejected", rejected_location, rejected_schema, rejected),
    ]
    return specs, {"source_count": source_count, "accepted": accepted, "rejected": rejected}


def curate_zones(raw_path: Path, work: Path) -> dict:
    location = "reference/taxi_zones/part-00000.parquet"
    schema = pa.schema([
        pa.field("LocationID", pa.int64(), nullable=False),
        pa.field("Borough", pa.string(), nullable=True),
        pa.field("Zone", pa.string(), nullable=True),
        pa.field("service_zone", pa.string(), nullable=True),
    ])
    columns = {field.name: [] for field in schema}
    with raw_path.open(encoding="utf-8-sig", newline="") as stream:
        for row in csv.DictReader(stream):
            columns["LocationID"].append(int(row["LocationID"]))
            for name in ("Borough", "Zone", "service_zone"):
                columns[name].append(row.get(name) or None)
    write_table(work / location, schema, columns)
    return table_spec("reference", "taxi_zones", location, schema, len(columns["LocationID"]))


def write_gold(work: Path, aggregate: dict) -> tuple[dict, dict]:
    location = "gold/daily_trips/part-00000.parquet"
    schema = pa.schema([
        pa.field("pickup_date", pa.string(), nullable=False),
        pa.field("service_type", pa.string(), nullable=False),
        pa.field("trip_count", pa.int64(), nullable=False),
        pa.field("total_amount_cents", pa.int64(), nullable=False),
        pa.field("total_trip_distance", pa.float64(), nullable=False),
    ])
    columns = {field.name: [] for field in schema}
    for (date, service), values in sorted(aggregate.items()):
        columns["pickup_date"].append(date)
        columns["service_type"].append(service)
        for name in ("trip_count", "total_amount_cents", "total_trip_distance"):
            columns[name].append(values[name])
    write_table(work / location, schema, columns)
    totals = {
        "gold_trip_count": sum(columns["trip_count"]),
        "gold_total_amount_cents": sum(columns["total_amount_cents"]),
        "gold_total_trip_distance": sum(columns["total_trip_distance"]),
    }
    return table_spec("gold", "daily_trips", location, schema, len(columns["pickup_date"])), totals


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, default=Path("/work"), help="local output directory")
    args = parser.parse_args()
    work = args.work_dir.resolve()
    raw = work / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    raw_paths = {name: raw / Path(url).name for name, url in SOURCES.items()}
    source_stats = {name: download(url, raw_paths[name]) for name, url in SOURCES.items()}

    aggregate = defaultdict(lambda: {"trip_count": 0, "total_amount_cents": 0, "total_trip_distance": 0.0})
    specs, yellow = curate_trips("yellow", raw_paths["yellow"], work, aggregate)
    green_specs, green = curate_trips("green", raw_paths["green"], work, aggregate)
    source_stats["yellow"].update(yellow)
    source_stats["green"].update(green)
    specs.extend(green_specs)
    specs.append(curate_zones(raw_paths["zones"], work))
    gold, gold_totals = write_gold(work, aggregate)
    specs.append(gold)
    manifest = {
        "tables": specs,
        "sources": source_stats,
        "gold_stats": gold_totals,
        "raw_files_excluded_from_catalog": [str(path.relative_to(work).as_posix()) for path in raw_paths.values()],
    }
    (work / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
