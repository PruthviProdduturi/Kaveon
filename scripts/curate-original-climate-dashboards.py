"""Materialize the original Vercel climate dashboard tables as Engine Parquet.

Inputs are the two CSVs consumed by ``data/climate-energy/load.sh``.  Output
names and columns follow the dashboard/dataset contract.  In particular the
cross-domain table uses ``avg_tc`` and ``max_tc``; the old Postgres view used
different aliases and could not execute the saved dashboard queries.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import subprocess
import tempfile
from collections import defaultdict
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq


ENERGY_COLUMNS = [
    "country", "year", "iso_code", "population", "gdp",
    "primary_energy_consumption", "energy_per_capita", "energy_per_gdp",
    "electricity_generation", "electricity_demand", "fossil_fuel_consumption",
    "fossil_share_energy", "renewables_consumption", "renewables_share_energy",
    "renewables_electricity", "solar_electricity", "wind_electricity",
    "hydro_electricity", "nuclear_electricity", "coal_electricity",
    "gas_electricity", "oil_electricity", "carbon_intensity_elec",
    "greenhouse_gas_emissions", "low_carbon_share_energy", "low_carbon_electricity",
]
INTEGER_COLUMNS = {"year", "population"}
TEXT_COLUMNS = {"country", "iso_code"}

PINNED_GIT_REF = "bee3de4"
PINNED_SOURCES = {
    "climate_temperature.csv": "5ae308a82e62f6ad6590748d333ac8441cfbd8590df3f0ec96cc76083753ec31",
    "energy_global.csv": "df2d09921db08c17b4fde048c7f2cb543f668dec3dbeb8d7cca8b99a2891e34e",
}


def restore_pinned_sources(destination: Path, git_ref: str = PINNED_GIT_REF) -> tuple[Path, Path]:
    """Restore the exact original input blobs retained in repository history."""
    destination.mkdir(parents=True, exist_ok=True)
    restored = []
    for name, expected_hash in PINNED_SOURCES.items():
        repo_path = f"data/climate-energy/{name}"
        try:
            payload = subprocess.check_output(["git", "show", f"{git_ref}:{repo_path}"])
        except (subprocess.CalledProcessError, FileNotFoundError) as exc:
            raise RuntimeError(f"cannot restore pinned climate source {git_ref}:{repo_path}") from exc
        actual_hash = hashlib.sha256(payload).hexdigest()
        if actual_hash != expected_hash:
            raise RuntimeError(f"pinned climate source hash mismatch for {name}: {actual_hash}")
        path = destination / name
        path.write_bytes(payload)
        restored.append(path)
    return restored[0], restored[1]


def number(value: str | None, integer: bool = False):
    if value is None or not value.strip():
        return None
    parsed = float(value)
    if not math.isfinite(parsed):
        return None
    return int(parsed) if integer else parsed


def schemas() -> tuple[pa.Schema, pa.Schema, pa.Schema]:
    temperature = pa.schema([
        pa.field("country", pa.string(), False), pa.field("country_code", pa.string()),
        pa.field("year", pa.int64(), False), pa.field("month", pa.int64(), False),
        pa.field("temp_change_c", pa.float64()),
    ])
    energy = pa.schema([
        pa.field(name, pa.string() if name in TEXT_COLUMNS else pa.int64() if name in INTEGER_COLUMNS else pa.float64())
        for name in ENERGY_COLUMNS
    ])
    cross_names = [
        "country", "iso_code", "year", "population", "gdp", "avg_tc", "max_tc", "min_tc",
        "primary_energy_consumption", "energy_per_capita", "electricity_generation", "electricity_demand",
        "fossil_share_energy", "renewables_share_energy", "renewables_electricity", "solar_electricity",
        "wind_electricity", "nuclear_electricity", "carbon_intensity_elec", "greenhouse_gas_emissions",
        "low_carbon_share_energy",
    ]
    cross = pa.schema([
        pa.field(name, pa.string() if name in {"country", "iso_code"} else pa.int64() if name in {"year", "population"} else pa.float64())
        for name in cross_names
    ])
    return temperature, energy, cross


def read_temperature(path: Path) -> list[dict]:
    rows = []
    with path.open(encoding="utf-8-sig", newline="") as stream:
        for source in csv.DictReader(stream):
            month = number(source.get("month"), True)
            if not source.get("country") or month is None or not 1 <= month <= 12:
                raise RuntimeError("temperature CSV contains an invalid country or month")
            rows.append({
                "country": source["country"].strip(), "country_code": (source.get("country_code") or "").strip() or None,
                "year": number(source.get("year"), True), "month": month,
                "temp_change_c": number(source.get("temp_change_c")),
            })
    return rows


def read_energy(path: Path) -> list[dict]:
    rows = []
    with path.open(encoding="utf-8-sig", newline="") as stream:
        for source in csv.DictReader(stream):
            missing = [name for name in ENERGY_COLUMNS if name not in source]
            if missing:
                raise RuntimeError(f"energy CSV is missing columns: {', '.join(missing)}")
            rows.append({name: (source[name].strip() or None) if name in TEXT_COLUMNS else number(source[name], name in INTEGER_COLUMNS) for name in ENERGY_COLUMNS})
    return rows


def materialize(temperature_csv: Path, energy_csv: Path, output: Path) -> dict:
    temperature_rows, energy_rows = read_temperature(temperature_csv), read_energy(energy_csv)
    temperature_schema, energy_schema, cross_schema = schemas()
    grouped: dict[tuple[str, int], list[float]] = defaultdict(list)
    for row in temperature_rows:
        if row["temp_change_c"] is not None:
            grouped[(row["country"], row["year"])].append(row["temp_change_c"])
    cross_rows = []
    for row in energy_rows:
        values = grouped.get((row["country"], row["year"]), [])
        cross_rows.append({
            "country": row["country"], "iso_code": row["iso_code"], "year": row["year"],
            "population": row["population"], "gdp": row["gdp"],
            "avg_tc": round(sum(values) / len(values), 3) if values else None,
            "max_tc": max(values) if values else None, "min_tc": min(values) if values else None,
            **{name: row[name] for name in cross_schema.names[8:]},
        })
    specs = []
    for name, rows, schema in (
        ("temperature_monthly", temperature_rows, temperature_schema),
        ("energy_annual", energy_rows, energy_schema),
        ("climate_x_energy", cross_rows, cross_schema),
    ):
        relative = f"climate_energy/{name}/part-00000.parquet"
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        pq.write_table(pa.Table.from_pylist(rows, schema=schema), destination, compression="snappy")
        specs.append({
            "schema": "climate_energy", "name": name, "location": relative, "row_count": len(rows),
            "columns": [{"name": field.name, "data_type": "Utf8" if pa.types.is_string(field.type) else "Int64" if pa.types.is_int64(field.type) else "Float64", "nullable": field.nullable} for field in schema],
        })
    manifest = {
        "format": "kaveon.original-climate-dashboard-data/v1", "tables": specs,
        "contract_note": "avg_tc/max_tc/min_tc follow saved dashboard and dataset metadata; this fixes the stale aliases in schema.sql.",
    }
    (output / "original-climate-dashboard-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--temperature-csv", type=Path)
    parser.add_argument("--energy-csv", type=Path)
    parser.add_argument("--from-pinned-git", action="store_true",
                        help=f"restore exact source CSV blobs from Git revision {PINNED_GIT_REF}")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.from_pinned_git:
        if args.temperature_csv or args.energy_csv:
            parser.error("--from-pinned-git cannot be combined with explicit CSV paths")
        with tempfile.TemporaryDirectory(prefix="kaveon-climate-") as temp:
            temperature_csv, energy_csv = restore_pinned_sources(Path(temp))
            manifest = materialize(temperature_csv, energy_csv, args.output)
    else:
        if not args.temperature_csv or not args.energy_csv:
            parser.error("provide both CSV paths or use --from-pinned-git")
        manifest = materialize(args.temperature_csv, args.energy_csv, args.output)
    print(f"Curated {sum(table['row_count'] for table in manifest['tables'])} rows across 3 exact climate tables")


if __name__ == "__main__":
    main()
