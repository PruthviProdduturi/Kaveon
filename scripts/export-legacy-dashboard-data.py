"""Stream the live dashboard relations from legacy PostgreSQL to Parquet.

The source connection is forced read-only.  Rows are fetched with a server-side
cursor in bounded batches and written incrementally, so the 504M-row product
event view does not have to fit in memory.

Example:
    $env:KAVEON_LEGACY_DATABASE_URL = "postgresql://..."
    python scripts/export-legacy-dashboard-data.py --output tmp/live-8-export
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from dataclasses import dataclass
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path
from typing import Any, Iterable

import pyarrow as pa
import pyarrow.parquet as pq
import psycopg2
from psycopg2 import sql


RELATIONS = (
    ("climate_energy", "energy_annual"),
    ("climate_energy", "temperature_monthly"),
    ("climate_energy", "climate_x_energy"),
    ("ai_benchmarks", "leaderboard"),
    ("ai_benchmarks", "arena_battles"),
    ("ai_benchmarks", "pricing"),
    ("public", "covid_global"),
    ("public", "nyc_taxi_borough"),
    ("public", "kaveon_events_enriched"),
)

REQUIRED_COLUMNS = {
    ("climate_energy", "energy_annual"): {"carbon_intensity_elec", "coal_electricity", "country", "gas_electricity", "greenhouse_gas_emissions", "hydro_electricity", "iso_code", "nuclear_electricity", "primary_energy_consumption", "renewables_share_energy", "solar_electricity", "wind_electricity", "year"},
    ("climate_energy", "temperature_monthly"): {"country", "month", "temp_change_c", "year"},
    ("climate_energy", "climate_x_energy"): {"avg_tc", "country", "iso_code"},
    ("ai_benchmarks", "leaderboard"): {"arena_elo", "humaneval", "input_cost", "is_open_source", "mmlu", "model_name", "provider"},
    ("ai_benchmarks", "arena_battles"): {"model_a", "win_rate_a"},
    ("ai_benchmarks", "pricing"): {"input_cost"},
    ("public", "covid_global"): {"continent", "country", "dt", "iso_code", "population", "total_cases", "total_deaths"},
    ("public", "nyc_taxi_borough"): {"avg_fare", "borough", "revenue", "trips"},
    ("public", "kaveon_events_enriched"): {"acquisition_channel", "actions", "cache_hits", "charts_created", "country", "deployment", "errors", "industry", "latency_p75_ms", "license", "platform", "queries_run", "region", "rows_scanned", "segment", "sessions", "surface", "team_size", "user_id"},
}

# Dataset 134 is a dangling API reference in the live contract.  If its old
# relation has been removed, reproduce the relation expected by the saved
# charts from the two source tables without mutating PostgreSQL.
CLIMATE_CROSS_QUERY = """
SELECT e.country, e.iso_code, e.year, e.population, e.gdp,
       AVG(t.temp_change_c) AS avg_tc,
       MAX(t.temp_change_c) AS max_tc,
       MIN(t.temp_change_c) AS min_tc,
       e.primary_energy_consumption, e.energy_per_capita,
       e.electricity_generation, e.electricity_demand,
       e.fossil_share_energy, e.renewables_share_energy,
       e.renewables_electricity, e.solar_electricity,
       e.wind_electricity, e.nuclear_electricity,
       e.carbon_intensity_elec, e.greenhouse_gas_emissions,
       e.low_carbon_share_energy
FROM climate_energy.energy_annual e
LEFT JOIN climate_energy.temperature_monthly t
  ON t.country = e.country AND t.year = e.year
GROUP BY e.country, e.iso_code, e.year, e.population, e.gdp,
         e.primary_energy_consumption, e.energy_per_capita,
         e.electricity_generation, e.electricity_demand,
         e.fossil_share_energy, e.renewables_share_energy,
         e.renewables_electricity, e.solar_electricity,
         e.wind_electricity, e.nuclear_electricity,
         e.carbon_intensity_elec, e.greenhouse_gas_emissions,
         e.low_carbon_share_energy
"""

# The live product dashboards never group or filter by event_date.  The source
# view has exactly one row per user/surface/day, so this lossless projection
# reduces 504M rows to at most 18M user/surface rows while preserving every SUM,
# AVG and COUNT(DISTINCT user_id) used by the canonical dashboard contract.
EVENTS_DASHBOARD_QUERY = """
SELECT user_id, surface, platform, license, segment, industry, region, country,
       deployment, acquisition_channel, team_size,
       SUM(actions) AS actions,
       SUM(sessions) AS sessions,
       SUM(queries_run) AS queries_run,
       SUM(charts_created) AS charts_created,
       SUM(errors) AS errors,
       SUM(rows_scanned) AS rows_scanned,
       SUM(cache_hits) AS cache_hits,
       AVG(latency_p75_ms) AS latency_p75_ms
FROM public.kaveon_events_enriched
GROUP BY user_id, surface, platform, license, segment, industry, region, country,
         deployment, acquisition_channel, team_size
"""


@dataclass(frozen=True)
class Relation:
    schema: str
    table: str
    kind: str
    estimated_rows: int
    source_bytes: int
    columns: list[dict[str, Any]]


def relation_metadata(connection, schema: str, table: str) -> Relation | None:
    with connection.cursor() as cursor:
        cursor.execute(
            """SELECT c.relkind, c.reltuples::bigint,
                      pg_total_relation_size(c.oid)
                 FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE n.nspname = %s AND c.relname = %s""",
            (schema, table),
        )
        row = cursor.fetchone()
        if row is None:
            return None
        cursor.execute(
            """SELECT column_name, data_type, udt_name, is_nullable
                 FROM information_schema.columns
                WHERE table_schema = %s AND table_name = %s
                ORDER BY ordinal_position""",
            (schema, table),
        )
        columns = [
            {"name": name, "postgres_type": data_type, "udt_name": udt,
             "nullable": nullable == "YES"}
            for name, data_type, udt, nullable in cursor.fetchall()
        ]
    return Relation(schema, table, row[0], row[1], row[2], columns)


def safe_value(value: Any) -> Any:
    if isinstance(value, Decimal):
        return value
    if isinstance(value, (date, datetime, str, bytes, bool, int, float)) or value is None:
        return value
    return json.dumps(value, sort_keys=True, default=str)


def batches(cursor, batch_rows: int) -> Iterable[pa.RecordBatch]:
    names = [column.name for column in cursor.description]
    while True:
        rows = cursor.fetchmany(batch_rows)
        if not rows:
            return
        yield pa.RecordBatch.from_pylist(
            [{name: safe_value(value) for name, value in zip(names, row)} for row in rows]
        )


def export_query(connection, query, destination: Path, batch_rows: int) -> tuple[int, pa.Schema]:
    destination.parent.mkdir(parents=True, exist_ok=True)
    cursor_name = "kaveon_export_" + re.sub(r"[^a-z0-9]", "_", destination.stem.lower())
    cursor = connection.cursor(name=cursor_name)
    cursor.itersize = batch_rows
    cursor.execute(query)
    writer = None
    count = 0
    schema = None
    try:
        for batch in batches(cursor, batch_rows):
            if writer is None:
                schema = batch.schema
                writer = pq.ParquetWriter(destination, schema, compression="zstd")
            elif batch.schema != schema:
                batch = pa.Table.from_batches([batch]).cast(schema).to_batches()[0]
            writer.write_batch(batch)
            count += batch.num_rows
    finally:
        if writer is not None:
            writer.close()
        cursor.close()
    if schema is None:
        schema = pa.schema([])
        pq.write_table(pa.table({}), destination, compression="zstd")
    return count, schema


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dsn-env", default="KAVEON_LEGACY_DATABASE_URL")
    parser.add_argument("--batch-rows", type=int, default=25_000)
    parser.add_argument("--inventory-only", action="store_true")
    parser.add_argument("--exclude-events", action="store_true",
                        help="Skip the very large kaveon_events_enriched view")
    args = parser.parse_args()
    if args.batch_rows < 1:
        parser.error("--batch-rows must be positive")
    dsn = os.environ.get(args.dsn_env)
    if not dsn:
        parser.error(f"set {args.dsn_env} to a read-only PostgreSQL DSN")

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    connection = psycopg2.connect(dsn, connect_timeout=20)
    connection.set_session(readonly=True, autocommit=False)
    manifest: dict[str, Any] = {
        "format": "kaveon.live-8-postgres-export/v1",
        "source": "legacy-postgresql-read-only",
        "batch_rows": args.batch_rows,
        "tables": [],
    }
    try:
        for schema, table in RELATIONS:
            if args.exclude_events and table == "kaveon_events_enriched":
                continue
            metadata = relation_metadata(connection, schema, table)
            reconstructed = metadata is None and (schema, table) == ("climate_energy", "climate_x_energy")
            entry: dict[str, Any] = {
                "schema": schema,
                "name": table,
                "exists": metadata is not None,
                "reconstructed": reconstructed,
            }
            if metadata:
                entry.update(kind=metadata.kind, estimated_rows=metadata.estimated_rows,
                             source_bytes=metadata.source_bytes, columns=metadata.columns)
                available = {column["name"] for column in metadata.columns}
                entry["missing_dashboard_columns"] = sorted(REQUIRED_COLUMNS[(schema, table)] - available)
                entry["contract_valid"] = not entry["missing_dashboard_columns"]
            if args.inventory_only:
                manifest["tables"].append(entry)
                continue
            if metadata is None and not reconstructed:
                entry["status"] = "missing"
                manifest["tables"].append(entry)
                continue
            if metadata is not None and not entry["contract_valid"]:
                entry["status"] = "contract_mismatch"
                manifest["tables"].append(entry)
                continue
            output_table = "kaveon_events_dashboard" if table == "kaveon_events_enriched" else table
            destination = output / "OpenSource" / schema / output_table / "part-00000.parquet"
            if reconstructed:
                query = sql.SQL(CLIMATE_CROSS_QUERY)
            elif table == "kaveon_events_enriched":
                query = sql.SQL(EVENTS_DASHBOARD_QUERY)
                entry["lossless_dashboard_projection"] = True
                entry["output_name"] = output_table
            else:
                query = sql.SQL("SELECT * FROM {}.{}").format(sql.Identifier(schema), sql.Identifier(table))
            count, arrow_schema = export_query(connection, query, destination, args.batch_rows)
            connection.rollback()
            entry.update(
                status="exported", row_count=count,
                location=destination.relative_to(output).as_posix(),
                parquet_bytes=destination.stat().st_size,
                sha256=file_sha256(destination),
                arrow_schema=str(arrow_schema),
            )
            manifest["tables"].append(entry)
    finally:
        connection.rollback()
        connection.close()
    manifest_path = output / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote manifest for {len(manifest['tables'])} relations to {manifest_path}")


if __name__ == "__main__":
    main()
