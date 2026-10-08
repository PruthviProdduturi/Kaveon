"""Append a day of events to the lake, and let the platform notice by itself.

This is the incremental half of the events dataset. It adds one day of rows to
`public.kaveon_events_enriched` as a Delta append and then stops: nothing
downstream is told to refresh, because everything downstream already watches
the table's source version.

    Delta append  ->  source version moves
                  ->  the coordinator folds the new files into the cube and the
                      statistics in the background (KAVEON_STATISTICS_AUTO_REFRESH,
                      on by default)
                  ->  freshness sees identity_sha256 change and reports
                      data_modified for the dataset
                  ->  the ask path rebuilds the dataset's DLM

**Append only.** This is a correctness requirement, not a style preference. The
cube folds *added* files cheaply — each new file is read once and its cells
merge in. But a *removed* file, when the shape declares a count_distinct
measure, cannot be folded out: a HyperLogLog sketch will not give a file's
values back, so every cell it touched has to be recomputed from the remaining
files, which means rebuilding the whole cube in one scan. That scan took 20
minutes on an 8-core host and cannot finish at all on a small one. So this
never rewrites or removes a file.

Each append is one Delta commit, so a reader sees the day before it or the day
after it, never half of it.

    python scripts/append-events.py --plan
    python scripts/append-events.py --apply
    python scripts/append-events.py --apply --date 2026-07-05 --users 250000

Requires `deltalake`, `pyarrow`, `numpy`, and an `az login` with write access to
the lake.
"""
import argparse
import shutil
import subprocess
import sys
from datetime import date, timedelta
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
from deltalake import DeltaTable, write_deltalake

# The table's own copy on disk. Delta needs the existing log to append to it,
# and building the commit locally means only the new objects are uploaded.
TABLE = Path(__file__).resolve().parents[1] / "data" / "adls-mirror" / "opensource" / \
    "kaveon" / "kaveon_product" / "kaveon_events_enriched_split"
ACCOUNT = "kaveonlake"
CONTAINER = "opensource"
REMOTE = "snapshots/2026-09-09-v1/public/kaveon_events_enriched"

# The value domains the existing 504,000,000 rows use. An appended day that
# invented a new region or licence would widen a cube axis and change what the
# dashboards report, so these are fixed to what the table already contains.
SURFACES = ["Chat", "Dashboard", "Chart Builder", "SQL Lab", "API", "Export"]
PLATFORMS = ["Web", "Desktop", "Mobile"]
LICENSES = ["Free", "Standard", "Professional", "Enterprise"]
SEGMENTS = ["Startup", "SMB", "Mid-Market", "Enterprise"]
INDUSTRIES = ["Technology", "Finance", "Healthcare", "Retail", "Manufacturing",
              "Education", "Energy", "Professional Services", "Real Estate", "Media"]
REGIONS = ["North America", "Europe", "Asia", "South America", "Africa", "Oceania"]
COUNTRIES = ["United States", "United Kingdom", "Germany", "Netherlands", "France",
             "Canada", "Brazil", "Chile", "India", "Japan", "Australia", "South Africa"]
DEPLOYMENTS = ["Cloud", "Hybrid", "On-Premise"]
CHANNELS = ["Direct", "Organic", "Paid", "Referral", "Partner"]
TEAM_SIZES = ["Solo", "Small", "Medium", "Large", "Enterprise"]

# Per-surface ranges, matching the shape of the original generator: each
# surface behaves differently, so a breakdown by surface stays meaningful.
SURFACE_RANGES = {
    "Chat":          ((3, 15), (1, 4), (60, 1800), (0, 3), (0, 0), (0, 1), (0, 1000), (0, 2), (100, 500)),
    "Dashboard":     ((5, 25), (1, 5), (120, 3600), (2, 10), (0, 1), (0, 2), (1000, 100000), (2, 8), (200, 1500)),
    "Chart Builder": ((3, 12), (1, 3), (180, 2400), (3, 15), (1, 5), (0, 2), (5000, 500000), (1, 5), (300, 2000)),
    "SQL Lab":       ((5, 20), (1, 4), (300, 3600), (5, 25), (0, 1), (0, 3), (10000, 1000000), (0, 3), (500, 3000)),
    "API":           ((10, 50), (1, 2), (30, 600), (1, 5), (0, 0), (0, 1), (100, 50000), (3, 10), (50, 500)),
    "Export":        ((1, 5), (1, 2), (30, 300), (1, 3), (0, 0), (0, 1), (50000, 1000000), (0, 2), (200, 1000)),
}

SCHEMA = pa.schema([
    pa.field("event_date", pa.string()), pa.field("user_id", pa.int64()),
    pa.field("surface", pa.string()), pa.field("actions", pa.int64()),
    pa.field("sessions", pa.int64()), pa.field("duration_sec", pa.int64()),
    pa.field("queries_run", pa.int64()), pa.field("charts_created", pa.int64()),
    pa.field("errors", pa.int64()), pa.field("rows_scanned", pa.int64()),
    pa.field("cache_hits", pa.int64()), pa.field("latency_p75_ms", pa.int64()),
    pa.field("platform", pa.string()), pa.field("license", pa.string()),
    pa.field("segment", pa.string()), pa.field("industry", pa.string()),
    pa.field("region", pa.string()), pa.field("country", pa.string()),
    pa.field("deployment", pa.string()), pa.field("acquisition_channel", pa.string()),
    pa.field("team_size", pa.string()),
])

AZ = shutil.which("az") or shutil.which("az.cmd") or "az.cmd"


def az(*args, timeout=3600):
    result = subprocess.run([AZ, *args], capture_output=True, text=True,
                            timeout=timeout, shell=False)
    return result.returncode, (result.stdout or "").strip(), (result.stderr or "").strip()


def _spread(values, users, seed):
    """Assign each user a value deterministically, so a user keeps their
    attributes from day to day the way a real dimension join would."""
    index = (users * seed) % len(values)
    lookup = np.array(values, dtype=object)
    return pa.array(lookup[index], type=pa.string())


def _between(users, low, high, seed):
    if high <= low:
        return pa.array(np.full(len(users), low, dtype=np.int64), type=pa.int64())
    return pa.array(low + np.abs((users * seed) % (high - low + 1)), type=pa.int64())


def build_day(day: str, users: int) -> pa.Table:
    """One day of events: every user active on every surface, as the existing
    rows are shaped."""
    uids = np.arange(1, users + 1, dtype=np.int64)
    salt = int(day.replace("-", "")) % 100_003
    blocks = []
    for position, surface in enumerate(SURFACES):
        actions, sessions, duration, queries, charts, errors, scanned, cache, latency = \
            SURFACE_RANGES[surface]
        step = 1 + position
        blocks.append(pa.table({
            "event_date": pa.array([day] * users, type=pa.string()),
            "user_id": pa.array(uids, type=pa.int64()),
            "surface": pa.array([surface] * users, type=pa.string()),
            "actions": _between(uids, *actions, 486187 + salt * step),
            "sessions": _between(uids, *sessions, 999331 + salt * step),
            "duration_sec": _between(uids, *duration, 1300813 + salt * step),
            "queries_run": _between(uids, *queries, 735391 + salt * step),
            "charts_created": _between(uids, *charts, 571373 + salt * step),
            "errors": _between(uids, *errors, 412619 + salt * step),
            "rows_scanned": _between(uids, *scanned, 2654435761 + salt * step),
            "cache_hits": _between(uids, *cache, 297179 + salt * step),
            "latency_p75_ms": _between(uids, *latency, 193939 + salt * step),
            "platform": _spread(PLATFORMS, uids, 7919),
            "license": _spread(LICENSES, uids, 6151),
            "segment": _spread(SEGMENTS, uids, 5281),
            "industry": _spread(INDUSTRIES, uids, 4409),
            "region": _spread(REGIONS, uids, 3571),
            "country": _spread(COUNTRIES, uids, 2693),
            "deployment": _spread(DEPLOYMENTS, uids, 1861),
            "acquisition_channel": _spread(CHANNELS, uids, 1013),
            "team_size": _spread(TEAM_SIZES, uids, 577),
        }).cast(SCHEMA))
    return pa.concat_tables(blocks)


def _add_actions(table: DeltaTable) -> list:
    """The table's file list as plain dicts. `get_add_actions` hands back an
    arro3 table, which travels through the Arrow PyCapsule interface rather
    than pyarrow's own API."""
    return pa.table(table.get_add_actions(flatten=True)).to_pylist()


def _day_range(action: dict):
    """The event_date span a file covers, from the statistics Delta records
    per file. A file with no statistics reports nothing rather than guessing."""
    return action.get("min.event_date"), action.get("max.event_date")


def already_present(table: DeltaTable, day: str) -> bool:
    """A day that is already in the table must not be appended twice: the cube
    folds additions in, so a duplicate day would silently double its cells."""
    for action in _add_actions(table):
        low, high = _day_range(action)
        if low is not None and low <= day <= (high or low):
            return True
    return False


def main():
    options = argparse.ArgumentParser()
    options.add_argument("--apply", action="store_true")
    options.add_argument("--date", default=None,
                         help="the day to append; defaults to the day after the table's last")
    options.add_argument("--users", type=int, default=100_000,
                         help="users active that day; 6 surface rows are written per user")
    args = options.parse_args()

    if not TABLE.is_dir():
        sys.exit(f"no local copy of the table at {TABLE}")
    table = DeltaTable(str(TABLE))
    before = table.version()

    day = args.date
    if day is None:
        last = max((_day_range(action)[1] or "") for action in _add_actions(table))
        day = str(date.fromisoformat(last) + timedelta(days=1)) if last else "2026-07-05"

    rows = args.users * len(SURFACES)
    print(f"table is at Delta version {before}")
    print(f"append {day}: {rows:,} rows ({args.users:,} users x {len(SURFACES)} surfaces)")
    if already_present(table, day):
        sys.exit(f"{day} is already in the table; appending it again would double its cells")
    if not args.apply:
        print("plan only; pass --apply to write")
        return

    batch = build_day(day, args.users)
    if batch.schema != SCHEMA:
        sys.exit("generated batch does not match the table's schema")
    before_files = {path.name for path in TABLE.glob("*.parquet")}
    # No schema_mode: the batch is cast to the table's schema above and any
    # drift should fail the append rather than quietly widen the table, which
    # is what a merge mode would do.
    write_deltalake(str(TABLE), batch, mode="append")

    table = DeltaTable(str(TABLE))
    after = table.version()
    if after != before + 1:
        sys.exit(f"expected Delta version {before + 1}, got {after}")
    added = sorted({path.name for path in TABLE.glob("*.parquet")} - before_files)
    print(f"committed Delta version {after}: {len(added)} new file(s)")

    # Upload the data first, while the published log still describes only the
    # old files, then the new log entry — which is the commit, and the moment
    # every reader sees the new day.
    for name in added:
        code, out, err = az("storage", "blob", "upload", "--account-name", ACCOUNT,
                            "--auth-mode", "login", "-c", CONTAINER,
                            "-n", f"{REMOTE}/{name}", "-f", str(TABLE / name),
                            "--overwrite", "--no-progress", "-o", "none")
        if code != 0:
            sys.exit(f"uploading {name} failed, nothing published: {(err or out)[:400]}")
    entry = f"{after:020d}.json"
    code, out, err = az("storage", "blob", "upload", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-c", CONTAINER,
                        "-n", f"{REMOTE}/_delta_log/{entry}",
                        "-f", str(TABLE / "_delta_log" / entry),
                        "--overwrite", "--no-progress", "-o", "none")
    if code != 0:
        sys.exit(f"the data is uploaded but the commit is not: {(err or out)[:400]}")

    total = pq.ParquetFile(TABLE / added[0]).metadata.num_rows if added else 0
    print(f"published {day} ({total:,} rows in the new file)")
    print("the coordinator folds it into the cube and statistics on its own; "
          "freshness will report the dataset as changed")


if __name__ == "__main__":
    main()
