"""Build and publish `public.kaveon_events_enriched`, the demo's flagship table.

Supersedes `scripts/build-kaveon-events-parquet.py` and `scripts/build_504m.py`.
It is the single definition of the table's schema, value domains, geography and
metric distributions; `scripts/append-events.py` imports those from here so an
appended day can never widen a cube axis or invent a country.

    python scripts/build_kaveon_events_table.py plan
    python scripts/build_kaveon_events_table.py build
    python scripts/build_kaveon_events_table.py convert
    python scripts/build_kaveon_events_table.py upload

`plan` validates the geography against the map the chart registers and prints
the file and weighting plan without writing anything. `build` writes the part
files. `convert` writes a Delta log over them. `upload` performs the lake
switch described under "Publishing" below. Building the cube is deliberately
*not* part of this tool: it needs the host resized and is a cost decision.

## What this build fixes relative to the published table

1. **`event_date` is `date32`, not text.** Stored as text, the Engine refuses a
   time axis on the cube (`time 'event_date' (Utf8) cannot be bucketed at day
   grain`), so every date-ranged question falls off the cube and scans the
   whole table. The column is a date here, and `DAY_OF_WEEK` seasonality is
   modelled so a time axis has something true to show.
2. **The metrics are drawn, not derived.** The previous generator computed
   `low + abs((user_id * seed) % (high - low + 1))`, which is a sawtooth over
   user_id: visibly patterned, uniform inside a hard range, and piling values
   onto the range bounds. Every metric here comes from a distribution chosen
   for that metric — lognormal with a long right tail for latency, scan volume
   and session length; Poisson for counts; a per-user intensity factor so power
   users exist. Determinism comes from seeding, not from arithmetic on the key,
   so the values are reproducible without being patterned.
3. **The world is covered.** 185 countries weighted by online population and
   technology adoption (`kaveon_events_geography`), against 26 before.

## Layout

`REMOTE_TASK_TIMEOUT` is a source constant in the Engine (600s) and the
planner distributes work per file, so a table in one large file is one task
that cannot finish: it is permanently un-cubeable. The 504,000,000 rows are
therefore written as 126 files, and every file holds exactly one `event_date`
so a day filter reads only that day's files. 28 days do not divide 126 evenly,
so the first 14 days are written as 5 files and the last 14 as 4 — 126 files,
504,000,000 rows, both exact.

Within a file there is one row group per surface, written in surface order, so
a row group's statistics bound `event_date` and `surface` exactly. A row group
is 600,000 or 750,000 rows, which keeps a reader's working set bounded on a
6 GB table and is what the writer streams through.

## Publishing

The publish order keeps readers on the old table until one object changes:

1. the new part files go up under names the current `_delta_log` does not
   mention, so every reader keeps seeing the published table during the upload;
2. the parts are listed back and each confirmed at its full byte size;
3. the current log objects are copied to the `backups` container;
4. the stale `…0001.json` append commit is deleted — the table falls back to
   its own version 0, which is complete and consistent, just one day shorter;
5. `_delta_log/00000000000000000000.json` is overwritten. That write is the
   switch.

Step 4 exists because the published log has two versions and the new log has
one. Leaving the version-1 commit in place would add a file written with the
old text `event_date` to a table whose schema now says `date`, which is not
readable. It is removed before the switch rather than after, so the table is
never in that state. The superseded part files are left in the lake: nothing
references them once the new log lands, and keeping them means the switch can
be reverted by restoring the two backed-up log objects alone.
"""
from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
import sys
import time
from datetime import date, timedelta
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kaveon_events_geography import (  # noqa: E402
    COUNTRIES, COUNTRY_NAMES, COUNTRY_REGION, REGIONS, latency_factors, shares,
    validate_against_geojson)

SEED = 20260704
N_USERS = 3_000_000
FIRST_DAY = date(2026, 7, 4)
DAYS = 28
FILES = 126
EXPECTED_ROWS = 504_000_000

REPO = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = (REPO / "data" / "adls-mirror" / "opensource" / "kaveon" /
                  "kaveon_product" / "kaveon_events_enriched_typed")

ACCOUNT = "kaveonlake"
CONTAINER = "opensource"
REMOTE = "snapshots/2026-09-09-v1/public/kaveon_events_enriched"
BACKUP_CONTAINER = "backups"
SWITCH_COMMIT = "00000000000000000000.json"

DISCLAIMER = "Deterministic demo telemetry; not production data."

# ── value domains ────────────────────────────────────────────────────────────
# Weights are the shares the published table already reports, so every existing
# breakdown keeps its shape. `append-events.py` imports these, which is what
# keeps an appended day from widening a cube axis — the previous appender held
# its own copy of the lists and had already drifted, adding an "industry" value
# ("Finance") the base table does not use.
WEIGHTED_DIMENSIONS: dict[str, tuple[tuple[str, int], ...]] = {
    "platform": (("Desktop", 6), ("Mobile", 5), ("Web", 5)),
    "license": (("Free", 3), ("Standard", 2), ("Professional", 2), ("Enterprise", 1)),
    "segment": (("Mid-Market", 3), ("Enterprise", 2), ("SMB", 2), ("Startup", 2)),
    "industry": (("Technology", 3), ("Healthcare", 1), ("Financial Services", 1),
                 ("Manufacturing", 1), ("Retail", 1), ("Education", 1), ("Media", 1),
                 ("Energy", 1), ("Government", 1), ("Logistics", 1),
                 ("Real Estate", 1), ("Professional Services", 1)),
    "deployment": (("Cloud", 4), ("Hybrid", 1), ("On-Premise", 1)),
    "acquisition_channel": (("Organic", 3), ("Referral", 2), ("Paid", 2),
                            ("Partner", 1), ("Direct", 1)),
    "team_size": (("Small", 3), ("Medium", 2), ("Solo", 2), ("Large", 1), ("Enterprise", 1)),
}

USER_DIMENSIONS = ("platform", "license", "segment", "industry", "region", "country",
                   "deployment", "acquisition_channel", "team_size")
METRICS = ("actions", "sessions", "duration_sec", "queries_run", "charts_created",
           "errors", "rows_scanned", "cache_hits", "latency_p75_ms")

SURFACES: tuple[str, ...] = ("Chat", "Dashboard", "Chart Builder", "SQL Lab", "API", "Export")

# Per surface, the parameters of each metric's distribution.
#   *_median / *_sigma  lognormal: `median` is the 50th percentile, `sigma` the
#                       log-scale spread, so a larger sigma is a longer right
#                       tail. Latency and scan volume are heavy-tailed in real
#                       telemetry; a uniform range is the one shape they are not.
#   *_lambda            Poisson: the mean of a count, so zero carries real mass
#                       where it should (errors, charts created outside the
#                       chart builder).
SURFACE_PROFILES: dict[str, dict[str, float]] = {
    "Chat": dict(actions_median=8, actions_sigma=0.70, sessions_lambda=1.4,
                 duration_median=420, duration_sigma=0.80, queries_lambda=1.2,
                 charts_lambda=0.02, errors_lambda=0.05,
                 scanned_median=180, scanned_sigma=1.90, cache_lambda=0.6,
                 latency_median=210, latency_sigma=0.45),
    "Dashboard": dict(actions_median=14, actions_sigma=0.65, sessions_lambda=1.9,
                      duration_median=900, duration_sigma=0.75, queries_lambda=5.0,
                      charts_lambda=0.35, errors_lambda=0.12,
                      scanned_median=24_000, scanned_sigma=1.50, cache_lambda=4.2,
                      latency_median=540, latency_sigma=0.50),
    "Chart Builder": dict(actions_median=7, actions_sigma=0.70, sessions_lambda=1.0,
                          duration_median=800, duration_sigma=0.70, queries_lambda=7.5,
                          charts_lambda=2.4, errors_lambda=0.18,
                          scanned_median=90_000, scanned_sigma=1.45, cache_lambda=2.1,
                          latency_median=760, latency_sigma=0.52),
    "SQL Lab": dict(actions_median=12, actions_sigma=0.75, sessions_lambda=1.5,
                    duration_median=1_200, duration_sigma=0.80, queries_lambda=12.0,
                    charts_lambda=0.15, errors_lambda=0.35,
                    scanned_median=320_000, scanned_sigma=1.50, cache_lambda=1.1,
                    latency_median=1_180, latency_sigma=0.58),
    "API": dict(actions_median=28, actions_sigma=0.80, sessions_lambda=0.4,
                duration_median=120, duration_sigma=0.85, queries_lambda=2.5,
                charts_lambda=0.01, errors_lambda=0.09,
                scanned_median=4_500, scanned_sigma=1.70, cache_lambda=6.3,
                latency_median=140, latency_sigma=0.42),
    "Export": dict(actions_median=3, actions_sigma=0.65, sessions_lambda=0.4,
                   duration_median=90, duration_sigma=0.70, queries_lambda=1.6,
                   charts_lambda=0.05, errors_lambda=0.07,
                   scanned_median=260_000, scanned_sigma=1.30, cache_lambda=0.5,
                   latency_median=430, latency_sigma=0.48),
}

# Activity is a working-week shape: a Saturday is well under half a Tuesday.
# Monday is index 0, matching `date.weekday()`.
WEEKDAY_FACTOR = (1.00, 1.04, 1.06, 1.03, 0.92, 0.44, 0.38)
# A mild adoption trend across the window, so a time axis has a direction.
TREND_OVER_WINDOW = 0.11

TEXT_COLUMNS = ("surface", *USER_DIMENSIONS)

# What a reader of the published files sees, and what `append-events.py` casts a
# new day to. `event_date` is the one column whose type changed.
SCHEMA = pa.schema(
    [pa.field("event_date", pa.date32()), pa.field("user_id", pa.int64()),
     pa.field("surface", pa.string())]
    + [pa.field(name, pa.int64()) for name in METRICS]
    + [pa.field(name, pa.string()) for name in USER_DIMENSIONS])

# How the columns are held while they are written. The text columns are
# dictionary-typed in memory so a file is built from indices rather than from
# 750,000 copies of a country name; `store_schema=False` keeps that typing out
# of the file's metadata, so readers receive plain Utf8 from the Parquet
# logical type while the pages stay dictionary-compressed.
WRITE_SCHEMA = pa.schema([
    field.with_type(pa.dictionary(pa.int32(), pa.string()))
    if field.name in TEXT_COLUMNS else field
    for field in SCHEMA])

# The metric columns hold irregular values, so a dictionary page would hold
# almost as many entries as there are rows. DELTA_BINARY_PACKED stores them as
# bit-packed deltas instead, which is what keeps a table of drawn values close
# to the size of the patterned one it replaces; `user_id` ascends inside every
# row group, where the same encoding is near-free. The dimensions stay
# dictionary-encoded: that is what they are.
INTEGER_ENCODING = {name: "DELTA_BINARY_PACKED" for name in ("user_id", *METRICS)}
DICTIONARY_COLUMNS = ["event_date", "surface", *USER_DIMENSIONS]
AZ = shutil.which("az") or shutil.which("az.cmd") or "az.cmd"


def az(*args, timeout=10_800):
    result = subprocess.run([AZ, *args], capture_output=True, text=True,
                            timeout=timeout, shell=False)
    return result.returncode, (result.stdout or "").strip(), (result.stderr or "").strip()


# ── the plan ─────────────────────────────────────────────────────────────────

def file_plan(days: int = DAYS, files: int = FILES,
              users: int = N_USERS) -> list[tuple[int, date, int, int]]:
    """`(day_offset, day, block_index, users_in_block)` per output file.

    Files are split across days as evenly as the counts allow, and never
    across a day: one file holds exactly one `event_date`, so a day filter
    reads only that day's files and a time axis prunes on file statistics
    alone.
    """
    base, extra = divmod(files, days)
    if base < 1:
        raise SystemExit(f"{files} files cannot cover {days} days")
    plan: list[tuple[int, date, int, int]] = []
    for offset in range(days):
        blocks = base + (1 if offset < extra else 0)
        if users % blocks:
            raise SystemExit(
                f"{users:,} users do not divide into {blocks} blocks for day {offset}; "
                f"choose a file count whose per-day split divides the user count")
        per_block = users // blocks
        for block in range(blocks):
            plan.append((offset, FIRST_DAY + timedelta(days=offset), block, per_block))
    return plan


def part_name(day: date, block: int) -> str:
    return f"events-{day.isoformat()}-{block}.parquet"


# ── user attributes ──────────────────────────────────────────────────────────

def _attribute_rng(name: str) -> np.random.Generator:
    """A generator per attribute, so adding an attribute never reshuffles
    another one's draw and the table stays reproducible column by column."""
    return np.random.default_rng([SEED, sum(ord(c) for c in name)])


def build_user_attributes(users: int = N_USERS) -> dict[str, object]:
    """Each user's fixed dimensions, drawn once and reused by every day.

    A user keeps their country, licence and team size from day to day, which
    is what a dimension join against a user table would produce. Each entry is
    `(values, indices)`: the dictionary and the per-user index into it.
    """
    weights = np.asarray(shares(), dtype=np.float64)
    country_index = _attribute_rng("country").choice(
        len(COUNTRIES), size=users, p=weights / weights.sum()).astype(np.int32)
    # `region` is derived from the country, never drawn, so the two columns
    # cannot disagree.
    region_of_country = np.asarray(
        [REGIONS.index(COUNTRY_REGION[name]) for name in COUNTRY_NAMES], dtype=np.int32)

    attributes: dict[str, object] = {
        "country": (list(COUNTRY_NAMES), country_index),
        "region": (list(REGIONS), region_of_country[country_index]),
    }
    for name, weighted in WEIGHTED_DIMENSIONS.items():
        values = [value for value, _ in weighted]
        shape = np.asarray([weight for _, weight in weighted], dtype=np.float64)
        attributes[name] = (values, _attribute_rng(name).choice(
            len(values), size=users, p=shape / shape.sum()).astype(np.int32))

    # How heavily a user uses the product, fixed for the window. Lognormal, so
    # a few users carry far more than the median and the distribution of any
    # volume metric has the long right tail real telemetry has.
    attributes["intensity"] = _attribute_rng("intensity").lognormal(
        mean=0.0, sigma=0.55, size=users).astype(np.float64)
    # Latency is a property of the path to the nearest region, so it follows
    # the user's country rather than their intensity, with a little per-user
    # jitter for the local network.
    factors = np.asarray(latency_factors())[country_index]
    attributes["latency_factor"] = (
        factors * _attribute_rng("latency jitter").lognormal(
            mean=0.0, sigma=0.12, size=users)).astype(np.float64)
    return attributes


# ── metrics ──────────────────────────────────────────────────────────────────

def chunk_rng(day_offset: int, block: int, surface_index: int) -> np.random.Generator:
    """Seeded by position, not by order, so any single file can be rebuilt
    byte-for-byte without rebuilding the ones before it."""
    return np.random.default_rng([SEED, day_offset, block, surface_index])


def surface_metrics(rng: np.random.Generator, profile: dict[str, float],
                    volume: np.ndarray, latency_factor: np.ndarray) -> dict[str, np.ndarray]:
    """One surface's metrics for one block of users on one day."""
    rows = volume.shape[0]

    def lognormal(median: float, sigma: float) -> np.ndarray:
        return rng.lognormal(mean=math.log(median), sigma=sigma, size=rows)

    # `actions` and `sessions` are never zero: a row exists because the user
    # was active on that surface that day.
    actions = 1 + np.floor(lognormal(profile["actions_median"],
                                     profile["actions_sigma"]) * volume)
    sessions = 1 + rng.poisson(profile["sessions_lambda"] * volume)
    duration = 15 + np.floor(lognormal(profile["duration_median"],
                                       profile["duration_sigma"]) * volume)
    # Errors rise with volume, but less than proportionally: a busier day is
    # not a uniformly worse one.
    errors = rng.poisson(profile["errors_lambda"] * (0.6 + 0.4 * volume))
    return {
        "actions": actions.astype(np.int64),
        "sessions": sessions.astype(np.int64),
        "duration_sec": duration.astype(np.int64),
        "queries_run": rng.poisson(profile["queries_lambda"] * volume).astype(np.int64),
        "charts_created": rng.poisson(profile["charts_lambda"] * volume).astype(np.int64),
        "errors": errors.astype(np.int64),
        "rows_scanned": np.floor(
            lognormal(profile["scanned_median"], profile["scanned_sigma"]) * volume
        ).astype(np.int64),
        "cache_hits": rng.poisson(profile["cache_lambda"] * volume).astype(np.int64),
        "latency_p75_ms": (18 + np.floor(
            lognormal(profile["latency_median"], profile["latency_sigma"]) * latency_factor
        )).astype(np.int64),
    }


def day_volume(day: date, day_offset: int, days: int) -> float:
    """The day's activity multiplier: working-week shape plus a mild trend."""
    trend = 1.0 + TREND_OVER_WINDOW * (day_offset / max(days - 1, 1))
    return WEEKDAY_FACTOR[day.weekday()] * trend


def build_file(path: Path, day: date, day_offset: int, block: int, per_block: int,
               attributes: dict[str, object], days: int) -> int:
    """One output file: one day, one block of users, all six surfaces.

    Written surface by surface so the writer's working set is one surface's
    rows rather than the whole file, and so each surface is its own row group.
    """
    first = block * per_block
    user_ids = pa.array(np.arange(first + 1, first + per_block + 1, dtype=np.int64))
    scale = day_volume(day, day_offset, days)
    volume = attributes["intensity"][first:first + per_block] * scale
    latency_factor = attributes["latency_factor"][first:first + per_block]
    event_date = pa.array(
        np.full(per_block, (day - date(1970, 1, 1)).days, dtype=np.int32)).cast(pa.date32())

    dimensions = {}
    for name in USER_DIMENSIONS:
        values, indices = attributes[name]
        dimensions[name] = pa.DictionaryArray.from_arrays(
            pa.array(indices[first:first + per_block]), pa.array(values, type=pa.string()))
    zero = pa.array(np.zeros(per_block, dtype=np.int32))

    written = 0
    writer = pq.ParquetWriter(
        path, WRITE_SCHEMA, compression="zstd", compression_level=3,
        use_dictionary=DICTIONARY_COLUMNS, column_encoding=INTEGER_ENCODING,
        data_page_version="2.0", write_statistics=True, store_schema=False)
    try:
        for surface_index, surface in enumerate(SURFACES):
            metrics = surface_metrics(
                chunk_rng(day_offset, block, surface_index),
                SURFACE_PROFILES[surface], volume, latency_factor)
            columns = {
                "event_date": event_date,
                "user_id": user_ids,
                "surface": pa.DictionaryArray.from_arrays(
                    zero, pa.array([surface], type=pa.string())),
            }
            columns.update({name: pa.array(metrics[name]) for name in METRICS})
            columns.update(dimensions)
            table = pa.table(columns, schema=WRITE_SCHEMA)
            writer.write_table(table, row_group_size=per_block)
            written += per_block
    finally:
        writer.close()
    return written


# ── steps ────────────────────────────────────────────────────────────────────

def plan(days: int, files: int, users: int) -> None:
    validate_against_geojson()
    weights = shares()
    ranked = sorted(zip(COUNTRY_NAMES, weights), key=lambda row: -row[1])
    layout = file_plan(days, files, users)
    rows = sum(per_block * len(SURFACES) for *_, per_block in layout)

    print(f"{len(COUNTRIES)} countries, every name a feature of the registered world map")
    print(f"{len(layout)} files over {days} days, {rows:,} rows "
          f"({users:,} users x {days} days x {len(SURFACES)} surfaces)")
    per_day: dict[date, int] = {}
    for _, day, _, _ in layout:
        per_day[day] = per_day.get(day, 0) + 1
    spread = sorted(set(per_day.values()))
    print(f"files per day: {spread} (a file never spans two days)")

    print("\ntop 15 countries by modelled share:")
    for name, share in ranked[:15]:
        print(f"  {name:<22} {share * 100:6.2f}%   {int(share * users):>9,} users")
    print("smallest 5:")
    for name, share in ranked[-5:]:
        print(f"  {name:<22} {share * 100:6.4f}%   {int(share * users):>9,} users")
    by_region: dict[str, float] = {}
    for name, share in zip(COUNTRY_NAMES, weights):
        by_region[COUNTRY_REGION[name]] = by_region.get(COUNTRY_REGION[name], 0.0) + share
    print("\nregion share:")
    for region, share in sorted(by_region.items(), key=lambda row: -row[1]):
        print(f"  {region:<16} {share * 100:5.1f}%")


def build(output: Path, days: int, files: int, users: int) -> None:
    validate_against_geojson()
    layout = file_plan(days, files, users)
    expected = sum(per_block * len(SURFACES) for *_, per_block in layout)
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)

    print(f"users: drawing {users:,} user attribute rows", flush=True)
    attributes = build_user_attributes(users)
    started = time.time()
    written = 0
    for index, (day_offset, day, block, per_block) in enumerate(layout, start=1):
        path = output / part_name(day, block)
        written += build_file(path, day, day_offset, block, per_block, attributes, days)
        if index % 7 == 0 or index == len(layout):
            elapsed = time.time() - started
            remaining = elapsed / index * (len(layout) - index)
            print(f"  {index}/{len(layout)} files, {written:,} rows, "
                  f"{elapsed / 60:.1f} min, ETA {remaining / 60:.0f} min", flush=True)

    verify(output, expected)
    manifest = {
        "table": "public.kaveon_events_enriched",
        "synthetic": True,
        "disclaimer": DISCLAIMER,
        "seed": SEED,
        "row_count": written,
        "file_count": len(layout),
        "first_day": FIRST_DAY.isoformat(),
        "days": days,
        "users": users,
        "surfaces": list(SURFACES),
        "columns": [{"name": field.name, "data_type": str(field.type)} for field in SCHEMA],
        "countries": [{"country": name, "region": COUNTRY_REGION[name],
                       "share": round(share, 10)}
                      for name, share in zip(COUNTRY_NAMES, shares())],
    }
    (output / "build-manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def verify(output: Path, expected: int | None = None) -> int:
    """Count what was actually written. Nothing is published until this agrees
    with the plan: a short file is a silent truncation of a headline number."""
    parts = sorted(output.glob("*.parquet"))
    counted = 0
    for part in parts:
        metadata = pq.ParquetFile(part).metadata
        counted += metadata.num_rows
    size = sum(part.stat().st_size for part in parts)
    print(f"{len(parts)} files, {counted:,} rows, {size / 1e9:.2f} GB")
    target = EXPECTED_ROWS if expected is None else expected
    if counted != target:
        raise SystemExit(f"row count is {counted:,}, expected {target:,}; refusing to continue")
    print(f"row count is exactly {target:,}")
    return counted


def convert(output: Path) -> None:
    from deltalake import convert_to_deltalake

    verify(output)
    log = output / "_delta_log"
    if log.exists():
        shutil.rmtree(log)
    convert_to_deltalake(str(output), mode="error")
    entries = sorted(log.glob("*.json"))
    if [entry.name for entry in entries] != [SWITCH_COMMIT]:
        raise SystemExit(f"expected one commit, got {[entry.name for entry in entries]}")
    print(f"wrote _delta_log/{SWITCH_COMMIT} ({entries[0].stat().st_size:,} bytes)")


def published_commits() -> list[str]:
    """The commit objects the lake currently holds for this table, ascending."""
    code, out, err = az("storage", "fs", "file", "list", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-f", CONTAINER,
                        "--path", f"{REMOTE}/_delta_log", "--recursive", "true",
                        "--query", "[?ends_with(name, 'json')].name", "-o", "tsv")
    if code != 0:
        raise SystemExit(f"could not read the published log: {(err or out)[:400]}")
    names = sorted(Path(line).name for line in out.splitlines() if line.strip())
    if not names:
        raise SystemExit("the published table has no Delta log; nothing to switch")
    return names


def write_padding_commits(output: Path, highest: int) -> list[Path]:
    """No-op commits for versions 1..`highest`, written beside the new log.

    A commit carrying only `commitInfo` is a legal Delta commit that applies no
    action, so the version exists and resolves to exactly what version 0 holds.
    It is what keeps the table's latest version from decreasing when a rebuild
    replaces a multi-commit log with a single one.
    """
    log = output / "_delta_log"
    written = []
    for version in range(1, highest + 1):
        entry = log / f"{version:020d}.json"
        entry.write_text(json.dumps({"commitInfo": {
            "timestamp": int(time.time() * 1000),
            "operation": "REBUILD",
            "operationParameters": {"version_held": str(version)},
            "engineInfo": "scripts/build_kaveon_events_table.py",
        }}) + "\n", encoding="utf-8")
        written.append(entry)
    return written


def upload(output: Path) -> None:
    log = output / "_delta_log" / SWITCH_COMMIT
    if not log.is_file():
        raise SystemExit("no _delta_log; run the convert step first")
    local = {part.name: part.stat().st_size for part in output.glob("*.parquet")}
    verify(output)

    # 1. The data, while the published log still names only the old objects.
    code, out, err = az("storage", "blob", "upload-batch", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-d", CONTAINER,
                        "--destination-path", REMOTE, "-s", str(output),
                        "--pattern", "*.parquet", "--overwrite", "--no-progress", "-o", "none")
    if code != 0:
        raise SystemExit(f"part upload failed, nothing published: {(err or out)[:500]}")
    print(f"uploaded {len(local)} part files")

    # 2. Every part at its full byte size before anything switches to them.
    code, out, err = az("storage", "fs", "file", "list", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-f", CONTAINER, "--path", REMOTE,
                        "--recursive", "true", "--query",
                        "[?ends_with(name, 'parquet')].{n:name,s:contentLength}", "-o", "tsv")
    if code != 0:
        raise SystemExit(f"could not list the uploaded parts: {(err or out)[:400]}")
    remote = {}
    for line in out.splitlines():
        name, _, size = line.rpartition("\t")
        remote[Path(name).name] = int(size)
    short = {name: (size, remote.get(name)) for name, size in local.items()
             if remote.get(name) != size}
    if short:
        raise SystemExit(f"{len(short)} part(s) did not land at full size: "
                         f"{dict(sorted(short.items())[:5])}")
    print(f"all {len(local)} parts verified byte-for-byte in the lake")

    # 3. The log the switch replaces, kept so the switch can be reverted.
    published = published_commits()
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    backup = f"kaveon_events_enriched/_delta_log-{stamp}"
    for entry in published:
        code, out, err = az(
            "storage", "blob", "copy", "start", "--account-name", ACCOUNT,
            "--auth-mode", "login", "--destination-container", BACKUP_CONTAINER,
            "--destination-blob", f"{backup}/{entry}",
            "--source-account-name", ACCOUNT, "--source-container", CONTAINER,
            "--source-blob", f"{REMOTE}/_delta_log/{entry}", "--requires-sync", "true",
            "-o", "none")
        if code != 0:
            raise SystemExit(f"could not back up {entry}, nothing switched: {(err or out)[:400]}")
    print(f"published log ({len(published)} commit(s)) backed up to "
          f"{BACKUP_CONTAINER}/{backup}")

    # 4. The rebuilt log is one commit and the published log may be several.
    #    **The latest Delta version must never go backwards.** The Engine pins
    #    a version per statement and resolves it on the worker, so removing a
    #    commit the coordinator has already seen fails every task with
    #    `storage: requested Delta version is not available` — the table stops
    #    reading entirely. So every published version above 0 is overwritten
    #    with a commit that changes nothing, and that happens *before* the
    #    switch: over the still-published version 0 a no-op simply drops what
    #    those commits had added, which leaves a consistent, readable table.
    pad = write_padding_commits(output, len(published) - 1)
    for entry in pad:
        code, out, err = az("storage", "blob", "upload", "--account-name", ACCOUNT,
                            "--auth-mode", "login", "-c", CONTAINER,
                            "-n", f"{REMOTE}/_delta_log/{entry.name}", "-f", str(entry),
                            "--overwrite", "--no-progress", "-o", "none")
        if code != 0:
            raise SystemExit(f"could not write {entry.name}, nothing switched: "
                             f"{(err or out)[:400]}")
    if pad:
        print(f"version held at {len(published) - 1} by {len(pad)} no-op commit(s)")

    # 5. The switch.
    code, out, err = az("storage", "blob", "upload", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-c", CONTAINER,
                        "-n", f"{REMOTE}/_delta_log/{SWITCH_COMMIT}", "-f", str(log),
                        "--overwrite", "--no-progress", "-o", "none")
    if code != 0:
        raise SystemExit(f"the data is uploaded but the switch did not land: {(err or out)[:400]}")
    print("switched: public.kaveon_events_enriched is now the rebuilt table")
    print("statistics and the cube are stale until ANALYZE with (cube = true) runs")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("step", choices=["plan", "build", "verify", "convert", "upload"])
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--days", type=int, default=DAYS)
    parser.add_argument("--files", type=int, default=FILES)
    parser.add_argument("--users", type=int, default=N_USERS)
    args = parser.parse_args()

    if args.step == "plan":
        plan(args.days, args.files, args.users)
    elif args.step == "build":
        build(args.output, args.days, args.files, args.users)
    elif args.step == "verify":
        verify(args.output, sum(per_block * len(SURFACES) for *_, per_block
                                in file_plan(args.days, args.files, args.users)))
    elif args.step == "convert":
        convert(args.output)
    elif args.step == "upload":
        upload(args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
