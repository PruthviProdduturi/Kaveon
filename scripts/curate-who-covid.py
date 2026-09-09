"""Curate the official WHO reported COVID-19 cases dataset into Engine Parquet tables.

The WHO file is named "daily" but WHO currently publishes reported counts weekly.
These are reported surveillance counts, not infection dates or estimates. The script
only writes beneath --work-dir (default /work), keeps raw/who-covid.csv, and writes
covid-manifest.json without touching any existing manifest.json.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import tempfile
import unittest
from collections import defaultdict
from datetime import date
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import requests


SOURCE_URL = "https://srhdpeuwpubsa.blob.core.windows.net/whdh/COVID/WHO-COVID-19-global-daily-data.csv"
BATCH_ROWS = 65_536
REPORTED_COLUMNS = (
    "report_date", "country_code", "country", "who_region",
    "new_cases", "cumulative_cases", "new_deaths", "cumulative_deaths",
)
COUNT_COLUMNS = {"new_cases", "cumulative_cases", "new_deaths", "cumulative_deaths"}


def schema() -> pa.Schema:
    return pa.schema([
        pa.field(name, pa.int64() if name in COUNT_COLUMNS else pa.string(), nullable=True)
        for name in REPORTED_COLUMNS
    ])


def table_spec(schema_name: str, name: str, location: str, table_schema: pa.Schema, rows: int) -> dict:
    return {
        "schema": schema_name,
        "name": name,
        "location": location,
        "columns": [
            {
                "name": field.name,
                "data_type": "Int64" if pa.types.is_int64(field.type) else "Utf8",
                "nullable": field.nullable,
            }
            for field in table_schema
        ],
        "row_count": rows,
    }


def download(destination: Path) -> dict:
    destination.parent.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256()
    size = 0
    with requests.get(SOURCE_URL, stream=True, timeout=(15, 120)) as response:
        response.raise_for_status()
        with destination.open("wb") as output:
            for chunk in response.iter_content(chunk_size=1024 * 1024):
                if chunk:
                    output.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
    return {"url": SOURCE_URL, "sha256": digest.hexdigest(), "bytes": size}


def normalized_row(source: dict[str, str]) -> dict[str, object]:
    values: dict[str, object] = {
        "report_date": source.get("Date_reported") or None,
        "country_code": source.get("Country_code") or None,
        "country": source.get("Country") or None,
        "who_region": source.get("WHO_region") or None,
    }
    for output, source_name in (
        ("new_cases", "New_cases"), ("cumulative_cases", "Cumulative_cases"),
        ("new_deaths", "New_deaths"), ("cumulative_deaths", "Cumulative_deaths"),
    ):
        raw = (source.get(source_name) or "").strip()
        values[output] = int(raw) if raw else None
    return values


def write_reported_cases(raw_path: Path, work: Path) -> tuple[dict, dict, dict]:
    output_schema = schema()
    location = "covid/reported_cases/part-00000.parquet"
    output_path = work / location
    output_path.parent.mkdir(parents=True, exist_ok=True)
    rows = 0
    minimum: str | None = None
    maximum: str | None = None
    latest: dict[str, dict[str, object]] = {}
    aggregate: dict[str, dict[str, int]] = defaultdict(
        lambda: {"new_cases": 0, "new_deaths": 0, "new_cases_present": 0, "new_deaths_present": 0}
    )
    columns = {field.name: [] for field in output_schema}

    def flush(writer: pq.ParquetWriter) -> None:
        if columns["report_date"]:
            writer.write_table(pa.Table.from_pydict(columns, schema=output_schema))
            for values in columns.values():
                values.clear()

    with raw_path.open(encoding="utf-8-sig", newline="") as source, pq.ParquetWriter(
        output_path, output_schema, compression="snappy"
    ) as writer:
        reader = csv.DictReader(source)
        required = {"Date_reported", "Country", "New_cases", "Cumulative_cases", "New_deaths", "Cumulative_deaths"}
        if not required.issubset(reader.fieldnames or []):
            raise ValueError("WHO source is missing required reported-count columns")
        for raw in reader:
            record = normalized_row(raw)
            report_date = record["report_date"]
            if not isinstance(report_date, str):
                raise ValueError("WHO row is missing Date_reported")
            date.fromisoformat(report_date)
            rows += 1
            minimum = report_date if minimum is None or report_date < minimum else minimum
            maximum = report_date if maximum is None or report_date > maximum else maximum
            for name in output_schema.names:
                columns[name].append(record[name])
            country = record["country"]
            if isinstance(country, str):
                previous = latest.get(country)
                if previous is None or report_date >= previous["report_date"]:
                    latest[country] = record
            date_aggregate = aggregate[report_date]
            for name in ("new_cases", "new_deaths"):
                value = record[name]
                if value is not None:
                    date_aggregate[name] += value
                    date_aggregate[f"{name}_present"] += 1
            if len(columns["report_date"]) >= BATCH_ROWS:
                flush(writer)
        flush(writer)
    if rows == 0 or minimum is None or maximum is None:
        raise ValueError("WHO source contains no reported rows")
    return (
        table_spec("covid", "reported_cases", location, output_schema, rows),
        latest,
        {"minimum": minimum, "maximum": maximum, "aggregate": aggregate},
    )


def write_latest(work: Path, latest: dict[str, dict[str, object]]) -> dict:
    location = "covid/country_latest/part-00000.parquet"
    output_schema = schema()
    ordered = [latest[country] for country in sorted(latest)]
    (work / location).parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(
        pa.Table.from_pylist(ordered, schema=output_schema), work / location,
        compression="snappy",
    )
    return table_spec("covid", "country_latest", location, output_schema, len(ordered))


def write_gold(work: Path, aggregate: dict[str, dict[str, int]]) -> dict:
    location = "gold/covid_reported_by_date/part-00000.parquet"
    output_schema = pa.schema([
        pa.field("report_date", pa.string(), nullable=False),
        pa.field("new_cases", pa.int64(), nullable=True),
        pa.field("new_deaths", pa.int64(), nullable=True),
    ])
    rows = []
    for report_date, values in sorted(aggregate.items()):
        rows.append({
            "report_date": report_date,
            "new_cases": values["new_cases"] if values["new_cases_present"] else None,
            "new_deaths": values["new_deaths"] if values["new_deaths_present"] else None,
        })
    (work / location).parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(pa.Table.from_pylist(rows, schema=output_schema), work / location, compression="snappy")
    return table_spec("gold", "covid_reported_by_date", location, output_schema, len(rows))


def curate(raw_path: Path, work: Path, source: dict) -> dict:
    work.mkdir(parents=True, exist_ok=True)
    reported, latest, coverage = write_reported_cases(raw_path, work)
    latest_spec = write_latest(work, latest)
    gold = write_gold(work, coverage.pop("aggregate"))
    checks = {spec["schema"] + "." + spec["name"]: spec["row_count"] for spec in (reported, latest_spec, gold)}
    if checks["covid.country_latest"] > checks["covid.reported_cases"]:
        raise RuntimeError("latest-country count exceeds reported-case rows")
    return {
        "tables": [reported, latest_spec, gold],
        "sources": {"who_covid": source},
        "date_coverage": coverage,
        "row_count_checks": checks,
        "notes": "WHO reported counts may be weekly; they are reported surveillance counts, not infection dates.",
    }


class CurationTests(unittest.TestCase):
    def test_fixture_preserves_negative_corrections_and_aggregates_new_counts(self) -> None:
        fixture = """Date_reported,Country_code,Country,WHO_region,New_cases,Cumulative_cases,New_deaths,Cumulative_deaths\n2024-01-01,AA,Alpha,EURO,10,100,2,20\n2024-01-08,AA,Alpha,EURO,-3,97,,20\n2024-01-08,BB,Beta,AFRO,5,5,1,1\n2024-01-15,CC,Gamma,EMRO,,0,,0\n"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / "raw.csv"
            raw.write_text(fixture, encoding="utf-8")
            manifest = curate(raw, root / "work", {"sha256": "fixture", "bytes": len(fixture)})
            reported = pq.read_table(root / "work/covid/reported_cases/part-00000.parquet")
            self.assertEqual(reported.column("new_cases").to_pylist()[1], -3)
            self.assertIsNone(reported.column("new_deaths").to_pylist()[1])
            gold = pq.read_table(root / "work/gold/covid_reported_by_date/part-00000.parquet")
            self.assertEqual(gold.column("new_cases").to_pylist(), [10, 2, None])
            self.assertEqual(gold.column("new_deaths").to_pylist(), [2, 1, None])
            self.assertEqual(manifest["row_count_checks"]["covid.country_latest"], 3)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, default=Path("/work"))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=["curate-who-covid.py"])
        return
    work = args.work_dir.resolve()
    raw_path = work / "raw" / "who-covid.csv"
    source = download(raw_path)
    manifest = curate(raw_path, work, source)
    (work / "covid-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"manifest": str(work / "covid-manifest.json"), "rows": manifest["row_count_checks"]}, sort_keys=True))


if __name__ == "__main__":
    main()
