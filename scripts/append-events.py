"""Append a day of events to the lake, and let the platform notice by itself.

This is the incremental half of the events dataset. It adds one day of rows to
`KaveonTelemetry.events.daily_user_activity` as a Delta append and then stops: nothing
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
    python scripts/append-events.py --apply --date 2026-08-01 --users 250000

`--users` is the size of the user population the day is drawn from, not the
number of rows: a user appears only on the surfaces they actually used, so the
row count is a consequence of the presence draws and is reported, never
assumed.

**The schema, the value domains, the presence model, the identifiers and every
metric distribution come from `build_kaveon_events_table`, not from this
file.** An appended day that invented
a region or a licence would widen a cube axis and change what the dashboards
report, and an earlier version of this script held its own copies of the lists
and had already drifted — it introduced an `industry` value ("Finance") the base
table does not use, and narrowed `country` from 26 values to 12. Importing the
domains is what makes that class of drift impossible rather than merely
discouraged.

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

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_kaveon_events_table import (  # noqa: E402
    DAYS, FIRST_DAY, METRIC_STREAM, SCHEMA, SURFACES, SURFACE_PROFILES,
    USER_DIMENSIONS, build_user_attributes, chunk_rng, day_shape,
    present_users, surface_metrics, user_ids)

# The table's own copy on disk. Delta needs the existing log to append to it,
# and building the commit locally means only the new objects are uploaded.
TABLE = Path(__file__).resolve().parents[1] / "data" / "adls-mirror" / "telemetry" / \
    "events" / "daily_user_activity"
ACCOUNT = "kaveonlake"
CONTAINER = "telemetry"
REMOTE = "events/daily_user_activity"

AZ = shutil.which("az") or shutil.which("az.cmd") or "az.cmd"


def az(*args, timeout=3600):
    result = subprocess.run([AZ, *args], capture_output=True, text=True,
                            timeout=timeout, shell=False)
    return result.returncode, (result.stdout or "").strip(), (result.stderr or "").strip()


def build_day(day: date, users: int) -> pa.Table:
    """One day of events, drawn exactly as the base table's days are drawn.

    The day's own offset from the table's first day carries into the weekday,
    trend and shock factors, so an appended Saturday is as quiet as a built one
    and the series does not step at the join. Presence is drawn the same way
    too: the rows are the user-surface pairs that were actually active, so an
    appended day is not a block of `users x surfaces` rows while every built
    day is a ragged one.
    """
    attributes = build_user_attributes(users)
    offset = (day - FIRST_DAY).days
    shape = day_shape(day, offset, DAYS)
    # `build_user_attributes` draws from a fixed-seed stream per attribute, so
    # the first `users` draws are the first `users` draws of the full table: a
    # user keeps the identifier, country and licence the built days gave them.
    identifiers = user_ids(attributes["user_uuid"], 0, users)
    epoch_day = (day - date(1970, 1, 1)).days

    blocks = []
    for surface_index, surface in enumerate(SURFACES):
        selected = present_users(offset, 0, surface_index, attributes, 0, users, shape)
        rows = selected.size
        if not rows:
            continue
        volume = (attributes["intensity"][selected] * shape).astype(np.float64)
        metrics = surface_metrics(
            chunk_rng(offset, 0, surface_index, METRIC_STREAM),
            SURFACE_PROFILES[surface], volume,
            attributes["latency_factor"][selected].astype(np.float64))
        columns = {
            "event_date": pa.array(
                np.full(rows, epoch_day, dtype=np.int32)).cast(pa.date32()),
            "user_id": pa.array([identifiers[position] for position in selected],
                                type=pa.string()),
            "surface": pa.array([surface] * rows, type=pa.string()),
        }
        columns.update({name: pa.array(values) for name, values in metrics.items()})
        for name in USER_DIMENSIONS:
            values, indices = attributes[name]
            columns[name] = pa.array(
                [values[index] for index in indices[selected]], type=pa.string())
        blocks.append(pa.table(columns, schema=SCHEMA))
    if not blocks:
        raise SystemExit(f"{day} drew no rows at all; check --users")
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


def _as_day(value) -> date | None:
    """`event_date` is a real date now, so Delta's per-file statistics come
    back as `date` objects rather than strings."""
    if value is None:
        return None
    return value if isinstance(value, date) else date.fromisoformat(str(value)[:10])


def already_present(table: DeltaTable, day: date) -> bool:
    """A day that is already in the table must not be appended twice: the cube
    folds additions in, so a duplicate day would silently double its cells."""
    for action in _add_actions(table):
        low, high = (_as_day(value) for value in _day_range(action))
        if low is not None and low <= day <= (high or low):
            return True
    return False


def last_day(table: DeltaTable) -> date | None:
    days = [_as_day(_day_range(action)[1]) for action in _add_actions(table)]
    present = [value for value in days if value is not None]
    return max(present) if present else None


def main():
    options = argparse.ArgumentParser()
    options.add_argument("--apply", action="store_true")
    options.add_argument("--date", default=None,
                         help="the day to append; defaults to the day after the table's last")
    options.add_argument("--users", type=int, default=100_000,
                         help="the user population the day is drawn from; each user "
                              "contributes a row only for the surfaces they used")
    args = options.parse_args()

    if not TABLE.is_dir():
        sys.exit(f"no local copy of the table at {TABLE}")
    table = DeltaTable(str(TABLE))
    before = table.version()

    if args.date:
        day = date.fromisoformat(args.date)
    else:
        latest = last_day(table)
        day = (latest + timedelta(days=1)) if latest else FIRST_DAY + timedelta(days=DAYS)

    print(f"table is at Delta version {before}")
    print(f"append {day}: drawn from a population of {args.users:,} users "
          f"across {len(SURFACES)} surfaces")
    if already_present(table, day):
        sys.exit(f"{day} is already in the table; appending it again would double its cells")

    batch = build_day(day, args.users)
    print(f"{batch.num_rows:,} rows drew present "
          f"({batch.num_rows / (args.users * len(SURFACES)) * 100:.1f}% of the "
          f"user x surface grid)")
    if not args.apply:
        print("plan only; pass --apply to write")
        return
    if batch.schema != SCHEMA:
        sys.exit("generated batch does not match the table's schema")
    before_files = {path.name for path in TABLE.glob("*.parquet")}
    # No schema_mode: the batch is built against the table's schema above and
    # any drift should fail the append rather than quietly widen the table,
    # which is what a merge mode would do.
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
