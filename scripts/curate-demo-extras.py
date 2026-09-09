"""Locally curate pinned OWID, NASA GISTEMP, and Open LLM leaderboard demos.

This script downloads only the listed public sources into /work/raw. It does
not upload, register Engine tables, or alter cloud resources.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
from datetime import date, datetime
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import requests


OWID_COMMIT = "https://api.github.com/repos/owid/energy-data/commits/master"
HF_CONTENTS = "https://huggingface.co/api/datasets/open-llm-leaderboard/contents"
NASA_GISTEMP = "https://data.giss.nasa.gov/gistemp/tabledata_v4/GLB.Ts+dSST.csv"
BATCH_ROWS = 32_768


def request_json(url: str):
    response = requests.get(url, timeout=(15, 60), headers={"User-Agent": "kaveon-demo-curator"})
    response.raise_for_status()
    return response.json(), response.headers


def github_revision() -> str:
    value, _ = request_json(OWID_COMMIT)
    sha = value.get("sha") if isinstance(value, dict) else None
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise RuntimeError("OWID commit API did not return a commit SHA")
    return sha


def huggingface_revision() -> str:
    value, headers = request_json(HF_CONTENTS)
    candidates = [headers.get("x-repo-commit"), headers.get("X-Repo-Commit")]
    if isinstance(value, dict):
        candidates.extend(value.get(key) for key in ("sha", "commit", "revision"))
    for candidate in candidates:
        if isinstance(candidate, str) and re.fullmatch(r"[0-9a-f]{40}", candidate):
            return candidate
    raise RuntimeError("Hugging Face contents API did not expose a repository commit SHA")


def download(url: str, destination: Path, provenance: str) -> dict:
    destination.parent.mkdir(parents=True, exist_ok=True)
    digest, size = hashlib.sha256(), 0
    with requests.get(url, stream=True, timeout=(15, 180), headers={"User-Agent": "kaveon-demo-curator"}) as response:
        response.raise_for_status()
        with destination.open("wb") as stream:
            for chunk in response.iter_content(chunk_size=1024 * 1024):
                if chunk:
                    stream.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
    return {"url": url, "sha256": digest.hexdigest(), "bytes": size, "provenance": provenance}


def snake_name(value: str, used: set[str]) -> str:
    name = re.sub(r"[^a-zA-Z0-9]+", "_", value).strip("_").lower()
    if not name:
        name = "field"
    if name[0].isdigit():
        name = "field_" + name
    stem, sequence = name, 2
    while name in used:
        name = f"{stem}_{sequence}"
        sequence += 1
    used.add(name)
    return name


def engine_type(field: pa.Field) -> pa.DataType:
    if pa.types.is_integer(field.type):
        return pa.int64()
    if pa.types.is_floating(field.type) or pa.types.is_decimal(field.type):
        return pa.float64()
    return pa.string()


def manifest_spec(schema_name: str, name: str, location: str, schema: pa.Schema, row_count: int) -> dict:
    def type_name(field: pa.Field) -> str:
        if pa.types.is_int64(field.type):
            return "Int64"
        if pa.types.is_float64(field.type):
            return "Float64"
        return "Utf8"
    return {
        "schema": schema_name,
        "name": name,
        "location": location,
        "columns": [{"name": field.name, "data_type": type_name(field), "nullable": field.nullable} for field in schema],
        "row_count": row_count,
    }


def json_value(value):
    if value is None:
        return None
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return value.hex()
    if isinstance(value, dict):
        return {str(key): json_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [json_value(item) for item in value]
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return value


def normalize(value, target: pa.DataType):
    if value is None:
        return None
    if pa.types.is_int64(target):
        return int(value)
    if pa.types.is_float64(target):
        number = float(value)
        return number if math.isfinite(number) else None
    if isinstance(value, str):
        return value
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, (dict, list, tuple, bytes)):
        return json.dumps(json_value(value), sort_keys=True, separators=(",", ":"))
    return str(value)


def write_rows(path: Path, schema: pa.Schema, rows) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    with pq.ParquetWriter(path, schema, compression="snappy") as writer:
        batch = {field.name: [] for field in schema}
        for row in rows:
            for field in schema:
                batch[field.name].append(row.get(field.name))
            count += 1
            if count % BATCH_ROWS == 0:
                writer.write_table(pa.Table.from_pydict(batch, schema=schema))
                batch = {field.name: [] for field in schema}
        if count % BATCH_ROWS:
            writer.write_table(pa.Table.from_pydict(batch, schema=schema))
    return count


def curate_energy(raw: Path, work: Path) -> dict:
    location = "climate_energy/energy/part-00000.parquet"
    with raw.open(encoding="utf-8", newline="") as stream:
        header = next(csv.reader(stream))
    fields = []
    for name in header:
        # OWID publishes country/ISO labels and year; numeric indicators are
        # represented as Float64 to avoid an inferred type changing by batch.
        kind = pa.int64() if name == "year" else pa.string() if name in {"country", "iso_code"} else pa.float64()
        fields.append(pa.field(snake_name(name, set(field.name for field in fields)), kind, nullable=True))
    schema = pa.schema(fields)

    def rows():
        with raw.open(encoding="utf-8", newline="") as stream:
            for source in csv.DictReader(stream):
                row = {}
                for original, field in zip(header, schema):
                    value = source.get(original, "")
                    if value == "":
                        row[field.name] = None
                    elif pa.types.is_int64(field.type):
                        row[field.name] = int(value)
                    elif pa.types.is_float64(field.type):
                        number = float(value)
                        row[field.name] = number if math.isfinite(number) else None
                    else:
                        row[field.name] = value
                yield row
    count = write_rows(work / location, schema, rows())
    return manifest_spec("climate_energy", "energy", location, schema, count)


def curate_codebook(raw: Path, work: Path) -> dict:
    location = "reference/energy_indicators/part-00000.parquet"
    schema = pa.schema([
        pa.field("indicator", pa.string(), nullable=False),
        pa.field("description", pa.string(), nullable=True),
    ])

    def rows():
        with raw.open(encoding="utf-8", newline="") as stream:
            for source in csv.DictReader(stream):
                yield {"indicator": source.get("column") or source.get("indicator") or source.get("Variable"),
                       "description": source.get("description") or source.get("Description")}
    count = write_rows(work / location, schema, rows())
    return manifest_spec("reference", "energy_indicators", location, schema, count)


def curate_gistemp(raw: Path, work: Path) -> dict:
    location = "climate_energy/global_temperature_monthly/part-00000.parquet"
    schema = pa.schema([
        pa.field("year", pa.int64(), nullable=False),
        pa.field("month", pa.int64(), nullable=False),
        pa.field("anomaly_deg_c", pa.float64(), nullable=True),
    ])
    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

    def rows():
        with raw.open(encoding="utf-8", newline="") as stream:
            next(stream, None)  # title; second line is Year, Jan...Dec
            for source in csv.DictReader(stream):
                try:
                    year = int((source.get("Year") or "").strip())
                except ValueError:
                    continue
                for index, name in enumerate(months, start=1):
                    value = (source.get(name) or "").strip()
                    yield {"year": year, "month": index,
                           "anomaly_deg_c": None if value in {"", "***"} else float(value)}
    count = write_rows(work / location, schema, rows())
    return manifest_spec("climate_energy", "global_temperature_monthly", location, schema, count)


def curate_huggingface(raw: Path, work: Path) -> dict:
    location = "ai_benchmarks/open_llm_results/part-00000.parquet"
    file = pq.ParquetFile(raw)
    used = set()
    names = [snake_name(field.name, used) for field in file.schema_arrow]
    schema = pa.schema([pa.field(name, engine_type(field), nullable=True)
                        for name, field in zip(names, file.schema_arrow)])
    count = 0
    path = work / location
    path.parent.mkdir(parents=True, exist_ok=True)
    with pq.ParquetWriter(path, schema, compression="snappy") as writer:
        for batch in file.iter_batches(batch_size=BATCH_ROWS):
            columns = {field.name: [] for field in schema}
            for original, target in zip(batch.columns, schema):
                columns[target.name].extend(normalize(value, target.type) for value in original.to_pylist())
            count += batch.num_rows
            writer.write_table(pa.Table.from_pydict(columns, schema=schema))
    return manifest_spec("ai_benchmarks", "open_llm_results", location, schema, count)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, default=Path("/work"))
    args = parser.parse_args()
    work = args.work_dir.resolve()
    raw = work / "raw"
    raw.mkdir(parents=True, exist_ok=True)

    owid_sha = github_revision()
    hf_sha = huggingface_revision()
    owid_base = f"https://raw.githubusercontent.com/owid/energy-data/{owid_sha}"
    hf_file = "data/train-00000-of-00001.parquet"
    sources = {
        "owid_energy": download(f"{owid_base}/owid-energy-data.csv", raw / "owid-energy-data.csv",
                                  f"OWID energy-data commit {owid_sha}"),
        "owid_codebook": download(f"{owid_base}/owid-energy-codebook.csv", raw / "owid-energy-codebook.csv",
                                    f"OWID energy-data commit {owid_sha}"),
        "nasa_gistemp": download(NASA_GISTEMP, raw / "GLB.Ts+dSST.csv",
                                  "NASA GISTEMP v4 global monthly anomaly; 1951-1980 baseline"),
        "open_llm_leaderboard": download(
            f"https://huggingface.co/datasets/open-llm-leaderboard/contents/resolve/{hf_sha}/{hf_file}",
            raw / "train-00000-of-00001.parquet", f"Hugging Face archived Open LLM Leaderboard contents commit {hf_sha}"),
    }
    tables = [
        curate_energy(raw / "owid-energy-data.csv", work),
        curate_codebook(raw / "owid-energy-codebook.csv", work),
        curate_gistemp(raw / "GLB.Ts+dSST.csv", work),
        curate_huggingface(raw / "train-00000-of-00001.parquet", work),
    ]
    manifest = {
        "tables": tables,
        "sources": sources,
        "notes": [
            "Open LLM Leaderboard contents is an archived snapshot, not current model rankings.",
            "All output values are normalized to Int64, Float64, or Utf8; nested leaderboard values are JSON strings.",
            "GISTEMP anomalies are degrees C relative to the 1951-1980 baseline; *** is preserved as null.",
            "raw/ contains originals and is excluded from the catalog manifest.",
        ],
        "raw_files_excluded_from_catalog": [str(path.relative_to(work).as_posix()) for path in raw.iterdir() if path.is_file()],
    }
    (work / "extras-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
