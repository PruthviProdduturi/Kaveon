"""Build and publish `KaveonTelemetry.events.activity`, the demo's flagship table.

Supersedes `scripts/build-kaveon-events-parquet.py` and `scripts/build_504m.py`.
It is the single definition of the table's schema, value domains, geography,
presence model and metric distributions; `scripts/append-events.py` imports
those from here so an appended day can never widen a cube axis, invent a
country, or draw its rows a different way.

    python scripts/build_kaveon_events_table.py plan
    python scripts/build_kaveon_events_table.py build
    python scripts/build_kaveon_events_table.py convert
    python scripts/build_kaveon_events_table.py upload

`plan` validates the geography against the map the chart registers, computes
the exact row count the seeds will produce, and prints the file and weighting
plan without writing anything. `build` writes the part files. `convert` writes
a Delta log over them. `upload` publishes to a *new* lake location and refuses
to write over one that already holds a table. Building the cube is deliberately
*not* part of this tool: it needs the host resized and is a cost decision.

## What this build fixes relative to the published table

The published `KaveonTelemetry.events.enriched` reads as fabricated, and three
specific properties are why (OPEN-DEFECTS D-4):

1. **Rows existed by construction, not by behaviour.** The generator wrote one
   row per (user, day, surface) for every user, every day, every surface — a
   complete cross product. That is where `504,000,000` came from
   (3,000,000 x 28 x 6), and why every one of the 28 days held exactly
   18,000,000 rows, weekends included. No telemetry looks like that: it says
   every user opened all six surfaces every single day of July.

   Here **presence is drawn**. A user appears on a surface on a day only if
   they were active there, with a probability built from their own engagement
   rate, their affinity for that surface, and the day's shape. So a day's row
   count is a consequence of behaviour, no two days are equal, the total is
   whatever the draws produce, and most users are absent from most surfaces on
   most days.

2. **`user_id` was a sequential integer.** `1, 2, 3 …` is not an identifier any
   real system emits, and it leaks two things a telemetry extract should not:
   the exact user count (`MAX(user_id)`) and the order users were created in
   (so `user_id < n` is a cohort filter nobody declared). `user_id` is now an
   opaque UUID v4 as text, derived from the seeded stream so it is stable for a
   user across every day and across an append, and uncorrelated with anything.

3. **The user count and totals were round.** `3,000,000` users is as much of a
   tell as `504,000,000` rows. `N_USERS` is a specific number, so
   `COUNT(DISTINCT user_id)` is specific too.

Two earlier fixes are kept:

4. **`event_date` is `date32`, not text.** Stored as text, the Engine refuses a
   time axis on the cube (`time 'event_date' (Utf8) cannot be bucketed at day
   grain`), so every date-ranged question falls off the cube and scans the
   whole table.
5. **The metrics are drawn, not derived.** An older generator computed
   `low + abs((user_id * seed) % (high - low + 1))`, a sawtooth over user_id:
   visibly patterned, uniform inside a hard range, and piling values onto the
   range bounds. Every metric here comes from a distribution chosen for that
   metric — lognormal with a long right tail for latency, scan volume and
   session length; Poisson for counts; a per-user intensity factor so power
   users exist. Determinism comes from seeding, not from arithmetic on the key.
6. **The world is covered.** 185 countries weighted by online population and
   technology adoption (`kaveon_events_geography`), against 26 before.

## The name

`enriched` is an ETL stage name — bronze/silver/gold vocabulary — and says
nothing about content. The catalog is already `KaveonTelemetry` and the schema
already `events`, so the table does not repeat either: a row is one user's
activity on one surface on one day, which is what `daily_user_activity` says,
which is what the rows are — one row per user per surface per day they were
active. **Renaming the live table is not this script's job**; see
`docs/engineering/events-activity-rebuild.md` for the migration.

## Layout

`REMOTE_TASK_TIMEOUT` is a source constant in the Engine (600s) and the planner
distributes work per file, so a table in one large file is one task that cannot
finish: it is permanently un-cubeable. The table is therefore written as `FILES`
files, and every file holds exactly one `event_date` so a day filter reads only
that day's files. Each file covers one contiguous block of users.

Because a Saturday now holds well under half a Tuesday's rows, the files are
*not* split evenly across the days: each day gets a share in proportion to how
busy it is, so a quiet Sunday takes four files and a busy Wednesday thirteen and
every file comes out about the same size. The per-task bound applies to the
largest file, so that is the one worth holding constant.

Within a file there is one row group per surface, written in surface order, so
a row group's statistics bound `event_date` and `surface` exactly.

## Publishing

`upload` writes a new table at a location that does not yet hold one:

1. the part files go up;
2. the parts are listed back and each confirmed at its full byte size;
3. `_delta_log/00000000000000000000.json` is written last. That write is what
   makes the table readable, so a reader never sees a partial table.

It refuses outright if the destination already has a `_delta_log`. The previous
version of this step performed an in-place switch over the published table —
backing up its log, neutralising its later commits, then overwriting version 0.
That machinery is gone on purpose: a rebuild that can overwrite a live table is
one mistake away from destroying the installation's context, and publishing to
a new name costs nothing but storage.
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

# Not a round number, and not a round number's factor. `COUNT(DISTINCT user_id)`
# is a headline figure on this table; `3,000,000` announced itself as generated
# before anyone looked at a single row.
N_USERS = 14_627_893

FIRST_DAY = date(2026, 7, 4)
# Eight weeks, not four. The target is about a billion rows, and the honest way
# to get there is a longer window rather than more users: doubling the user
# count would double how many people are in the product without changing how
# any of them behave, while eight weeks gives a trend chart something to show
# and keeps the per-day volumes where they were.
DAYS = 56

# One day never spans two files' worth of days, and the busiest file stays near
# the per-task cost the published table had. `user_id` as text is ~3x the width
# of an int64, so the same row count is a larger file than before: more files,
# not bigger ones.
FILES = 504

REPO = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO / "data" / "adls-mirror" / "telemetry" / "events" / "daily_user_activity"

# Where a fresh build publishes. The telemetry container, under the table's own
# name — never over `events/enriched`, which is what the installation is
# reading right now.
ACCOUNT = "kaveonlake"
CONTAINER = "telemetry"
REMOTE = "events/daily_user_activity"
COMMIT = "00000000000000000000.json"

CATALOG = "KaveonTelemetry"
SCHEMA_NAME = "events"
TABLE_NAME = "daily_user_activity"

DISCLAIMER = "Deterministic demo telemetry; not production data."

# ── value domains ────────────────────────────────────────────────────────────
# Weights are the shares the published table already reports, so every existing
# breakdown keeps its shape. `append-events.py` imports these, which is what
# keeps an appended day from widening a cube axis — the previous appender held
# its own copy of the lists and had already drifted, adding an "industry" value
# ("Finance") the base table does not use.
# Each dimension's share of users, in basis points of its own total.
#
# These were small integers — 1, 2, 3 — and small integers over a dozen values
# produce identical shares: eleven industries at exactly 7.1% each, Mobile and
# Web both at 31.2%, two team sizes at 22.2% and two more at 11.1%. A breakdown
# chart then draws one tall bar and a row of bars the same height, which is the
# same tell as a table whose every day held exactly 18,000,000 rows. Nothing in
# the world distributes itself that evenly.
#
# So the weights are differentiated and roughly defensible, and no two values
# within a dimension are equal. They are approximations of a plausible
# software-buying population rather than measurements, and are labelled as such
# — the point is a credible shape, not a sourced statistic.
#
# `test_build_kaveon_events_table` asserts no two weights in a dimension match,
# so a future edit cannot quietly reintroduce the flat tail.
WEIGHTED_DIMENSIONS: dict[str, tuple[tuple[str, int], ...]] = {
    # A data product is used at a desk. Mobile is real but light: reading a
    # dashboard on a phone, not building one.
    "platform": (("Desktop", 4600), ("Web", 3300), ("Mobile", 2100)),
    # A freemium funnel: most accounts never pay, and the paid tiers thin out
    # as they get more expensive.
    "license": (("Free", 5200), ("Standard", 2400), ("Professional", 1600),
                ("Enterprise", 800)),
    "segment": (("SMB", 3400), ("Mid-Market", 2800), ("Startup", 2300),
                ("Enterprise", 1500)),
    # Weighted towards where analytics is actually bought — technology and
    # financial services lead, and the tail is a tail rather than a plateau.
    "industry": (("Technology", 1900), ("Financial Services", 1300),
                 ("Healthcare", 1000), ("Manufacturing", 900), ("Retail", 800),
                 ("Professional Services", 750), ("Education", 700),
                 ("Government", 600), ("Media", 500), ("Logistics", 450),
                 ("Energy", 400), ("Real Estate", 300)),
    "deployment": (("Cloud", 7100), ("Hybrid", 1900), ("On-Premise", 1000)),
    "acquisition_channel": (("Organic", 3700), ("Paid", 2300), ("Referral", 1800),
                            ("Partner", 1300), ("Direct", 900)),
    "team_size": (("Small", 3100), ("Solo", 2900), ("Medium", 2200),
                  ("Large", 1200), ("Enterprise", 600)),
}

USER_DIMENSIONS = ("platform", "license", "segment", "industry", "region", "country",
                   "deployment", "acquisition_channel", "team_size")
METRICS = ("actions", "sessions", "duration_sec", "queries_run", "charts_created",
           "errors", "rows_scanned", "cache_hits", "latency_p75_ms")

SURFACES: tuple[str, ...] = ("Chat", "Dashboard", "Chart Builder", "SQL Lab", "API", "Export")

# Per surface, `presence` plus the parameters of each metric's distribution.
#   presence            the share of a fully engaged user's days on which this
#                       surface is touched at all. It is what decides whether a
#                       row exists, so it is the one parameter that moves the
#                       table's row count.
#   *_median / *_sigma  lognormal: `median` is the 50th percentile, `sigma` the
#                       log-scale spread, so a larger sigma is a longer right
#                       tail. Latency and scan volume are heavy-tailed in real
#                       telemetry; a uniform range is the one shape they are not.
#   *_lambda            Poisson: the mean of a count, so zero carries real mass
#                       where it should (errors, charts created outside the
#                       chart builder).
SURFACE_PROFILES: dict[str, dict[str, float]] = {
    "Chat": dict(presence=0.62,
                 actions_median=8, actions_sigma=0.70, sessions_lambda=1.4,
                 duration_median=420, duration_sigma=0.80, queries_lambda=1.2,
                 charts_lambda=0.02, errors_lambda=0.05,
                 scanned_median=180, scanned_sigma=1.90, cache_lambda=0.6,
                 latency_median=210, latency_sigma=0.45),
    "Dashboard": dict(presence=0.74,
                      actions_median=14, actions_sigma=0.65, sessions_lambda=1.9,
                      duration_median=900, duration_sigma=0.75, queries_lambda=5.0,
                      charts_lambda=0.35, errors_lambda=0.12,
                      scanned_median=24_000, scanned_sigma=1.50, cache_lambda=4.2,
                      latency_median=540, latency_sigma=0.50),
    "Chart Builder": dict(presence=0.21,
                          actions_median=7, actions_sigma=0.70, sessions_lambda=1.0,
                          duration_median=800, duration_sigma=0.70, queries_lambda=7.5,
                          charts_lambda=2.4, errors_lambda=0.18,
                          scanned_median=90_000, scanned_sigma=1.45, cache_lambda=2.1,
                          latency_median=760, latency_sigma=0.52),
    "SQL Lab": dict(presence=0.33,
                    actions_median=12, actions_sigma=0.75, sessions_lambda=1.5,
                    duration_median=1_200, duration_sigma=0.80, queries_lambda=12.0,
                    charts_lambda=0.15, errors_lambda=0.35,
                    scanned_median=320_000, scanned_sigma=1.50, cache_lambda=1.1,
                    latency_median=1_180, latency_sigma=0.58),
    "API": dict(presence=0.28,
                actions_median=28, actions_sigma=0.80, sessions_lambda=0.4,
                duration_median=120, duration_sigma=0.85, queries_lambda=2.5,
                charts_lambda=0.01, errors_lambda=0.09,
                scanned_median=4_500, scanned_sigma=1.70, cache_lambda=6.3,
                latency_median=140, latency_sigma=0.42),
    "Export": dict(presence=0.11,
                   actions_median=3, actions_sigma=0.65, sessions_lambda=0.4,
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
# A day's own idiosyncratic shock. Without it the series is a perfect weekly
# sawtooth with a ruler through it, which is its own kind of tell: every
# Wednesday would sit exactly on the trend line. Lognormal and small, drawn
# once per day from the day's offset, so it is reproducible per day.
DAY_SHOCK_SIGMA = 0.045

# A user's engagement: the share of ordinary weekdays on which they are active
# at all. Beta, so it is bounded in (0, 1) with a long body — plenty of
# occasional users, a core who are there most days, nobody at exactly 100%.
ENGAGEMENT_ALPHA = 1.9
ENGAGEMENT_BETA = 2.1
# Most users have a home surface and visit the others occasionally. The home
# surface's presence is multiplied by this, the rest by the user's `breadth`.
PRIMARY_SURFACE_BONUS = 2.6
BREADTH_SIGMA = 0.50
# Nobody is active every single day on any surface: a probability of 1.0 would
# reintroduce the cross product for the heaviest users.
MAX_PRESENCE = 0.97

TEXT_COLUMNS = ("surface", *USER_DIMENSIONS)

# What a reader of the published files sees, and what `append-events.py` casts a
# new day to.
SCHEMA = pa.schema(
    [pa.field("event_date", pa.date32()), pa.field("user_id", pa.string()),
     pa.field("surface", pa.string())]
    + [pa.field(name, pa.int64()) for name in METRICS]
    + [pa.field(name, pa.string()) for name in USER_DIMENSIONS])

# How the columns are held while they are written. The low-cardinality text
# columns are dictionary-typed in memory so a file is built from indices rather
# than from two million copies of a country name; `store_schema=False` keeps
# that typing out of the file's metadata, so readers receive plain Utf8 from the
# Parquet logical type while the pages stay dictionary-compressed. `user_id` is
# not among them: it is unique per row inside a row group, so a dictionary would
# hold one entry per row and buy nothing.
WRITE_SCHEMA = pa.schema([
    field.with_type(pa.dictionary(pa.int32(), pa.string()))
    if field.name in TEXT_COLUMNS else field
    for field in SCHEMA])

# The metric columns hold irregular values, so a dictionary page would hold
# almost as many entries as there are rows. DELTA_BINARY_PACKED stores them as
# bit-packed deltas instead, which is what keeps a table of drawn values close
# to the size of the patterned one it replaces. The dimensions stay
# dictionary-encoded: that is what they are. `user_id` gets neither — a v4 UUID
# shares no prefix with its neighbour, so delta and prefix encodings both lose
# to plain bytes under zstd.
INTEGER_ENCODING = {name: "DELTA_BINARY_PACKED" for name in METRICS}
DICTIONARY_COLUMNS = ["event_date", "surface", *USER_DIMENSIONS]
AZ = shutil.which("az") or shutil.which("az.cmd") or "az.cmd"


def az(*args, timeout=10_800):
    result = subprocess.run([AZ, *args], capture_output=True, text=True,
                            timeout=timeout, shell=False)
    return result.returncode, (result.stdout or "").strip(), (result.stderr or "").strip()


# ── the plan ─────────────────────────────────────────────────────────────────

def files_per_day(days: int = DAYS, files: int = FILES) -> list[int]:
    """How many files each day gets, in proportion to how busy the day is.

    A day's row count is no longer uniform, so an even split of the files
    would make a Tuesday file nearly three times a Sunday file — and the
    per-file cost is what the Engine's 600s task bound applies to, so the
    constraint is on the *largest* file, not the average one. Allocating
    proportionally to the day's shape makes every file about the same size
    whatever the day.

    `day_shape` needs no draws, so this is computable before any data exists
    and `verify` derives the same plan the build used.
    """
    if files < days:
        raise SystemExit(f"{files} files cannot cover {days} days")
    weights = [day_shape(FIRST_DAY + timedelta(days=offset), offset, days)
               for offset in range(days)]
    scale = (files - days) / sum(weights)
    # One file per day is the floor; the rest are handed out by largest
    # remainder so the counts sum to `files` exactly.
    allocated = [1 + int(weight * scale) for weight in weights]
    remainders = sorted(range(days),
                        key=lambda offset: -((weights[offset] * scale) % 1.0))
    for offset in remainders[:files - sum(allocated)]:
        allocated[offset] += 1
    if sum(allocated) != files:
        raise SystemExit(f"allocated {sum(allocated)} files, expected {files}")
    return allocated


def file_plan(days: int = DAYS, files: int = FILES,
              users: int = N_USERS) -> list[tuple[int, date, int, int, int]]:
    """`(day_offset, day, block_index, first_user, users_in_block)` per file.

    A file never spans two days: one file holds exactly one `event_date`, so a
    day filter reads only that day's files and a time axis prunes on file
    statistics alone. Users are split across a day's files as evenly as the
    counts allow.

    Remainders are spread, not refused. The previous version required the user
    count to divide the per-day file count exactly, which is a constraint on
    nothing real — and it is part of why the table ended up with a round
    3,000,000 users.
    """
    allocated = files_per_day(days, files)
    plan: list[tuple[int, date, int, int, int]] = []
    for offset in range(days):
        blocks = allocated[offset]
        per_block, spare = divmod(users, blocks)
        if per_block < 1:
            raise SystemExit(f"{users:,} users cannot fill {blocks} blocks on day {offset}")
        first = 0
        for block in range(blocks):
            count = per_block + (1 if block < spare else 0)
            plan.append((offset, FIRST_DAY + timedelta(days=offset), block, first, count))
            first += count
        if first != users:
            raise SystemExit(f"day {offset} covers {first:,} users, expected {users:,}")
    return plan


def part_name(day: date, block: int) -> str:
    return f"daily-user-activity-{day.isoformat()}-{block:03d}.parquet"


# ── user attributes ──────────────────────────────────────────────────────────

def _attribute_rng(name: str) -> np.random.Generator:
    """A generator per attribute, so adding an attribute never reshuffles
    another one's draw and the table stays reproducible column by column.

    Every draw below fills its output in C order from this one stream, so the
    first `n` users of a large draw are the first `n` users of a small one.
    That is what lets `append-events.py` rebuild a prefix of the user table and
    get the same people back.
    """
    return np.random.default_rng([SEED, sum(ord(c) for c in name)])


def _uuid_bytes(users: int) -> np.ndarray:
    """16 random bytes per user, shaped into a UUID v4.

    Drawn as full-range `uint32` words, which consume exactly one word of the
    stream each and therefore keep the prefix property above; a bounded draw
    uses rejection sampling and would not.
    """
    words = _attribute_rng("user id").integers(
        0, 1 << 32, size=(users, 4), dtype=np.uint32)
    raw = words.view(np.uint8).reshape(users, 16)
    # Version 4 in the high nibble of byte 6, RFC 4122 variant in byte 8. A
    # value that is merely 16 random bytes is not a UUID, and anything that
    # parses UUIDs would reject it.
    raw[:, 6] = (raw[:, 6] & 0x0F) | 0x40
    raw[:, 8] = (raw[:, 8] & 0x3F) | 0x80
    return raw


def user_ids(raw: np.ndarray, first: int, count: int) -> list[str]:
    """Canonical 8-4-4-4-12 text for a block of users.

    Hyphenated text rather than 16 raw bytes because this column is read by
    people and by SQL — `WHERE user_id = '…'` has to be typeable — and because
    the dataset declares it as a varchar dimension.
    """
    hexed = raw[first:first + count].tobytes().hex()
    return [f"{group[0:8]}-{group[8:12]}-{group[12:16]}-{group[16:20]}-{group[20:32]}"
            for group in (hexed[at:at + 32] for at in range(0, len(hexed), 32))]


def build_user_attributes(users: int = N_USERS) -> dict[str, object]:
    """Each user's fixed properties, drawn once and reused by every day.

    A user keeps their identifier, country, licence and team size from day to
    day, which is what a dimension join against a user table would produce —
    the dimensions are a property of the user, not of the row, so they are
    never re-rolled per row. Each dimension entry is `(values, indices)`: the
    dictionary and the per-user index into it. Indices are held in the narrowest
    integer that fits the domain, because this table now has enough users that
    the attribute arrays are the build's memory ceiling.
    """
    weights = np.asarray(shares(), dtype=np.float64)
    country_index = _attribute_rng("country").choice(
        len(COUNTRIES), size=users, p=weights / weights.sum()).astype(np.int16)
    # `region` is derived from the country, never drawn, so the two columns
    # cannot disagree.
    region_of_country = np.asarray(
        [REGIONS.index(COUNTRY_REGION[name]) for name in COUNTRY_NAMES], dtype=np.int8)

    attributes: dict[str, object] = {
        "user_uuid": _uuid_bytes(users),
        "country": (list(COUNTRY_NAMES), country_index),
        "region": (list(REGIONS), region_of_country[country_index]),
    }
    for name, weighted in WEIGHTED_DIMENSIONS.items():
        values = [value for value, _ in weighted]
        shape = np.asarray([weight for _, weight in weighted], dtype=np.float64)
        attributes[name] = (values, _attribute_rng(name).choice(
            len(values), size=users, p=shape / shape.sum()).astype(np.int8))

    # How heavily a user uses the product when they are there, fixed for the
    # window. Lognormal, so a few users carry far more than the median and the
    # distribution of any volume metric has the long right tail real telemetry
    # has.
    attributes["intensity"] = _attribute_rng("intensity").lognormal(
        mean=0.0, sigma=0.55, size=users).astype(np.float32)
    # How often they are there at all. This is what makes a row's existence a
    # fact about a person rather than a fact about the loop that wrote the file.
    attributes["engagement"] = _attribute_rng("engagement").beta(
        ENGAGEMENT_ALPHA, ENGAGEMENT_BETA, size=users).astype(np.float32)
    # Everyone has a surface they live in; `breadth` is how much of the rest of
    # the product they also touch. Without this, every user's surface mix is the
    # same mix and `surface` carries no per-user signal at all.
    presence = np.asarray([SURFACE_PROFILES[surface]["presence"]
                           for surface in SURFACES], dtype=np.float64)
    attributes["primary_surface"] = _attribute_rng("primary surface").choice(
        len(SURFACES), size=users, p=presence / presence.sum()).astype(np.int8)
    breadth = _attribute_rng("breadth").lognormal(
        mean=0.0, sigma=BREADTH_SIGMA, size=users)
    # Centred on 1.0 so `breadth` redistributes attention between surfaces
    # instead of quietly inflating the table's row count.
    attributes["breadth"] = (breadth / math.exp(BREADTH_SIGMA ** 2 / 2)).astype(np.float32)

    # Latency is a property of the path to the nearest region, so it follows
    # the user's country rather than their intensity, with a little per-user
    # jitter for the local network.
    factors = np.asarray(latency_factors())[country_index]
    attributes["latency_factor"] = (
        factors * _attribute_rng("latency jitter").lognormal(
            mean=0.0, sigma=0.12, size=users)).astype(np.float32)
    return attributes


# ── presence and metrics ─────────────────────────────────────────────────────

# Two independent streams per chunk. Presence has to be replayable without
# replaying the metrics, so `verify` can recount a 500,000,000-row table from
# the seeds in under a minute instead of regenerating it.
PRESENCE_STREAM = 0
METRIC_STREAM = 1


def chunk_rng(day_offset: int, block: int, surface_index: int,
              stream: int) -> np.random.Generator:
    """Seeded by position, not by order, so any single file can be rebuilt
    byte-for-byte without rebuilding the ones before it."""
    return np.random.default_rng([SEED, stream, day_offset, block, surface_index])


def day_shape(day: date, day_offset: int, days: int) -> float:
    """The day's activity multiplier: working-week shape, trend, and the day's
    own shock. It scales both how many rows the day has and how busy they are,
    which is the coherent thing: a quiet Sunday is fewer people doing less."""
    trend = 1.0 + TREND_OVER_WINDOW * (day_offset / max(days - 1, 1))
    shock = float(np.random.default_rng([SEED, 9_001, day_offset]).lognormal(
        mean=0.0, sigma=DAY_SHOCK_SIGMA))
    return WEEKDAY_FACTOR[day.weekday()] * trend * shock


def presence_probability(surface_index: int, attributes: dict[str, object],
                         first: int, count: int, shape: float) -> np.ndarray:
    """Per user, the chance of touching this surface on this day."""
    surface = SURFACES[surface_index]
    base = SURFACE_PROFILES[surface]["presence"]
    engagement = attributes["engagement"][first:first + count]
    breadth = attributes["breadth"][first:first + count]
    primary = attributes["primary_surface"][first:first + count]
    affinity = np.where(primary == surface_index, PRIMARY_SURFACE_BONUS, breadth)
    return np.minimum(engagement * affinity * base * shape, MAX_PRESENCE)


def present_users(day_offset: int, block: int, surface_index: int,
                  attributes: dict[str, object], first: int, count: int,
                  shape: float) -> np.ndarray:
    """The positions, within this block, of the users who were active on this
    surface on this day. A Bernoulli draw per user — which is why no two days
    hold the same number of rows and why the total is whatever it is."""
    probability = presence_probability(surface_index, attributes, first, count, shape)
    draw = chunk_rng(day_offset, block, surface_index, PRESENCE_STREAM).random(
        count, dtype=np.float32)
    return np.flatnonzero(draw < probability)


def surface_metrics(rng: np.random.Generator, profile: dict[str, float],
                    volume: np.ndarray, latency_factor: np.ndarray) -> dict[str, np.ndarray]:
    """One surface's metrics for the users who were present."""
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


def planned_rows(day_offset: int, day: date, block: int, first: int, count: int,
                 attributes: dict[str, object], days: int) -> list[int]:
    """The row count this file will hold, per surface, from the seeds alone.

    Replays only the presence draws, which is cheap, so the row count is a
    prediction the build is held to rather than a number read back out of
    whatever got written.
    """
    shape = day_shape(day, day_offset, days)
    return [present_users(day_offset, block, index, attributes, first, count, shape).size
            for index in range(len(SURFACES))]


def build_file(path: Path, day: date, day_offset: int, block: int, first: int,
               count: int, attributes: dict[str, object], days: int,
               seen: np.ndarray | None = None) -> int:
    """One output file: one day, one block of users, every surface they used.

    Written surface by surface so the writer's working set is one surface's
    rows rather than the whole file, and so each surface is its own row group.
    """
    shape = day_shape(day, day_offset, days)
    block_uuids = user_ids(attributes["user_uuid"], first, count)
    epoch_day = (day - date(1970, 1, 1)).days

    written = 0
    writer = pq.ParquetWriter(
        path, WRITE_SCHEMA, compression="zstd", compression_level=3,
        use_dictionary=DICTIONARY_COLUMNS, column_encoding=INTEGER_ENCODING,
        data_page_version="2.0", write_statistics=True, store_schema=False)
    try:
        for surface_index, surface in enumerate(SURFACES):
            selected = present_users(day_offset, block, surface_index,
                                     attributes, first, count, shape)
            rows = selected.size
            if not rows:
                continue
            if seen is not None:
                seen[first + selected] = True
            volume = attributes["intensity"][first:first + count][selected] * shape
            metrics = surface_metrics(
                chunk_rng(day_offset, block, surface_index, METRIC_STREAM),
                SURFACE_PROFILES[surface], volume.astype(np.float64),
                attributes["latency_factor"][first:first + count][selected].astype(np.float64))
            columns = {
                "event_date": pa.array(
                    np.full(rows, epoch_day, dtype=np.int32)).cast(pa.date32()),
                "user_id": pa.array([block_uuids[position] for position in selected],
                                    type=pa.string()),
                "surface": pa.DictionaryArray.from_arrays(
                    pa.array(np.zeros(rows, dtype=np.int32)),
                    pa.array([surface], type=pa.string())),
            }
            columns.update({name: pa.array(metrics[name]) for name in METRICS})
            for name in USER_DIMENSIONS:
                values, indices = attributes[name]
                columns[name] = pa.DictionaryArray.from_arrays(
                    pa.array(indices[first:first + count][selected].astype(np.int32)),
                    pa.array(values, type=pa.string()))
            writer.write_table(pa.table(columns, schema=WRITE_SCHEMA), row_group_size=rows)
            written += rows
    finally:
        writer.close()
    return written


# ── steps ────────────────────────────────────────────────────────────────────

def projection(layout, attributes, users: int,
               days: int) -> tuple[dict[date, int], dict[str, int], int]:
    """Rows per day, rows per surface and the distinct user count, from the
    seeds alone — before a single row is written.

    The distinct user count is reported because it is a headline figure on this
    table and the one the old build got most obviously wrong. It is lower than
    the population: a user who was never active in the window has no rows, and
    so does not appear.
    """
    per_day: dict[date, int] = {}
    per_surface = {surface: 0 for surface in SURFACES}
    seen = np.zeros(users, dtype=bool)
    for day_offset, day, block, first, count in layout:
        shape = day_shape(day, day_offset, days)
        for index, surface in enumerate(SURFACES):
            selected = present_users(day_offset, block, index, attributes,
                                     first, count, shape)
            per_day[day] = per_day.get(day, 0) + selected.size
            per_surface[surface] += selected.size
            seen[first + selected] = True
    return per_day, per_surface, int(seen.sum())


def report_shape(per_day: dict[date, int], users: int,
                 distinct_users: int | None = None) -> None:
    """Print the properties this rebuild exists to establish, and fail if any
    of them does not hold. A table that is flat, round, or weekend-blind is the
    defect returning, and it should be caught here rather than by a reader."""
    days = sorted(per_day)
    counts = [per_day[day] for day in days]
    total = sum(counts)
    print(f"\n{total:,} rows over {len(days)} days, "
          f"drawn from {users:,} users")
    if distinct_users is not None:
        print(f"COUNT(DISTINCT user_id) will be {distinct_users:,}; the "
              f"{users - distinct_users:,} users with no activity in the window "
              "have no rows")
    print(f"mean {total / len(days):,.0f} rows/day, "
          f"min {min(counts):,} ({days[counts.index(min(counts))]}), "
          f"max {max(counts):,} ({days[counts.index(max(counts))]})")
    for day in days:
        bar = "#" * round(per_day[day] / max(counts) * 44)
        print(f"  {day} {day.strftime('%a')} {per_day[day]:>12,}  {bar}")

    if len(set(counts)) != len(counts):
        raise SystemExit("two days hold exactly the same number of rows; "
                         "the presence model is not being applied per day")
    weekday = [count for day, count in zip(days, counts) if day.weekday() < 5]
    weekend = [count for day, count in zip(days, counts) if day.weekday() >= 5]
    if weekend and min(weekday) <= max(weekend):
        raise SystemExit("a weekend day is busier than a weekday; the working-week "
                         "shape is not reaching row counts")
    if total % 1_000_000 == 0:
        raise SystemExit(f"the total is exactly {total:,}; a round headline figure is "
                         "the defect this build exists to remove")


def plan(days: int, files: int, users: int) -> None:
    validate_against_geojson()
    weights = shares()
    ranked = sorted(zip(COUNTRY_NAMES, weights), key=lambda row: -row[1])
    layout = file_plan(days, files, users)

    print(f"{len(COUNTRIES)} countries, every name a feature of the registered world map")
    print(f"{len(layout)} files over {days} days (a file never spans two days)")
    per_day_files: dict[date, int] = {}
    for _, day, _, _, _ in layout:
        per_day_files[day] = per_day_files.get(day, 0) + 1
    print(f"files per day: {sorted(set(per_day_files.values()))}")

    print("\ndrawing user attributes to project the row count", flush=True)
    attributes = build_user_attributes(users)
    started = time.time()
    per_day, per_surface, distinct_users = projection(layout, attributes, users, days)
    total = sum(per_day.values())
    print(f"projected in {time.time() - started:.0f}s")
    report_shape(per_day, users, distinct_users)

    print("\nrows by surface:")
    for surface, rows in sorted(per_surface.items(), key=lambda row: -row[1]):
        print(f"  {surface:<14} {rows:>12,}  {rows / total * 100:5.1f}%")

    print("\ntop 15 countries by modelled share:")
    for name, share in ranked[:15]:
        print(f"  {name:<22} {share * 100:6.2f}%   {int(share * users):>10,} users")
    print("smallest 5:")
    for name, share in ranked[-5:]:
        print(f"  {name:<22} {share * 100:6.4f}%   {int(share * users):>10,} users")
    by_region: dict[str, float] = {}
    for name, share in zip(COUNTRY_NAMES, weights):
        by_region[COUNTRY_REGION[name]] = by_region.get(COUNTRY_REGION[name], 0.0) + share
    print("\nregion share:")
    for region, share in sorted(by_region.items(), key=lambda row: -row[1]):
        print(f"  {region:<16} {share * 100:5.1f}%")


def build(output: Path, days: int, files: int, users: int) -> None:
    validate_against_geojson()
    layout = file_plan(days, files, users)
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)

    print(f"users: drawing {users:,} user attribute rows", flush=True)
    attributes = build_user_attributes(users)
    started = time.time()
    written = 0
    per_day: dict[date, int] = {}
    # Which users ended up with at least one row. `COUNT(DISTINCT user_id)` is a
    # headline figure on this table, so the build states it rather than leaving
    # it to be discovered by a query over half a billion rows.
    seen = np.zeros(users, dtype=bool)
    for index, (day_offset, day, block, first, count) in enumerate(layout, start=1):
        path = output / part_name(day, block)
        rows = build_file(path, day, day_offset, block, first, count, attributes,
                          days, seen)
        written += rows
        per_day[day] = per_day.get(day, 0) + rows
        if index % 14 == 0 or index == len(layout):
            elapsed = time.time() - started
            remaining = elapsed / index * (len(layout) - index)
            print(f"  {index}/{len(layout)} files, {written:,} rows, "
                  f"{elapsed / 60:.1f} min, ETA {remaining / 60:.0f} min", flush=True)

    distinct_users = int(seen.sum())
    report_shape(per_day, users, distinct_users)
    counted = verify(output, days, files, users, attributes=attributes)
    manifest = {
        "table": f"{CATALOG}.{SCHEMA_NAME}.{TABLE_NAME}",
        "synthetic": True,
        "disclaimer": DISCLAIMER,
        "seed": SEED,
        "row_count": counted,
        "file_count": len(layout),
        "first_day": FIRST_DAY.isoformat(),
        "days": days,
        "users": users,
        "distinct_users": distinct_users,
        "surfaces": list(SURFACES),
        "rows_per_day": {day.isoformat(): rows for day, rows in sorted(per_day.items())},
        "columns": [{"name": field.name, "data_type": str(field.type)}
                    for field in SCHEMA],
        "countries": [{"country": name, "region": COUNTRY_REGION[name],
                       "share": round(share, 10)}
                      for name, share in zip(COUNTRY_NAMES, shares())],
    }
    (output / "build-manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def verify(output: Path, days: int = DAYS, files: int = FILES, users: int = N_USERS,
           attributes: dict[str, object] | None = None) -> int:
    """Hold what was written to what the seeds say should have been written.

    Per file and per row group, not just in total: a short row group inside a
    file of the right length is exactly the kind of silent truncation a single
    grand total hides. The expected counts come from replaying the presence
    draws, so this is a real check and not a restatement of a constant.
    """
    layout = file_plan(days, files, users)
    if attributes is None:
        print(f"users: drawing {users:,} user attribute rows to recount", flush=True)
        attributes = build_user_attributes(users)

    counted = 0
    for day_offset, day, block, first, count in layout:
        part = output / part_name(day, block)
        if not part.is_file():
            raise SystemExit(f"{part.name} is missing; refusing to continue")
        expected = [rows for rows in planned_rows(day_offset, day, block, first, count,
                                                  attributes, days) if rows]
        metadata = pq.ParquetFile(part).metadata
        actual = [metadata.row_group(index).num_rows
                  for index in range(metadata.num_row_groups)]
        if actual != expected:
            raise SystemExit(
                f"{part.name}: row groups are {actual}, the seeds say {expected}; "
                "the file is not what this build would produce")
        counted += metadata.num_rows

    extra = sorted(part.name for part in output.glob("*.parquet")
                   if part.name not in {part_name(day, block)
                                        for _, day, block, _, _ in layout})
    if extra:
        raise SystemExit(f"{len(extra)} part file(s) are not in the plan: {extra[:5]}")

    size = sum(part.stat().st_size for part in output.glob("*.parquet"))
    print(f"{len(layout)} files, {counted:,} rows, {size / 1e9:.2f} GB "
          f"({size / max(counted, 1):.1f} bytes/row)")
    return counted


def convert(output: Path, days: int, files: int, users: int) -> None:
    from deltalake import convert_to_deltalake

    verify(output, days, files, users)
    log = output / "_delta_log"
    if log.exists():
        shutil.rmtree(log)
    convert_to_deltalake(str(output), mode="error")
    entries = sorted(log.glob("*.json"))
    if [entry.name for entry in entries] != [COMMIT]:
        raise SystemExit(f"expected one commit, got {[entry.name for entry in entries]}")
    print(f"wrote _delta_log/{COMMIT} ({entries[0].stat().st_size:,} bytes)")


def destination_is_empty() -> bool:
    """Whether the remote location already holds a table.

    A rebuild must never be able to overwrite a published table: the
    installation's datasets, cube and DLM context are built on the one it is
    reading, and regenerating underneath them has destroyed that before.
    """
    code, out, err = az("storage", "fs", "file", "list", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-f", CONTAINER,
                        "--path", f"{REMOTE}/_delta_log", "--recursive", "true",
                        "--query", "[].name", "-o", "tsv")
    if code != 0:
        text = (err or out).lower()
        if "notfound" in text or "does not exist" in text or "not found" in text:
            return True
        raise SystemExit(f"could not inspect the destination: {(err or out)[:400]}")
    return not [line for line in out.splitlines() if line.strip()]


def upload(output: Path, days: int, files: int, users: int) -> None:
    log = output / "_delta_log" / COMMIT
    if not log.is_file():
        raise SystemExit("no _delta_log; run the convert step first")
    if not destination_is_empty():
        raise SystemExit(
            f"{CONTAINER}/{REMOTE} already holds a Delta table. This step only ever "
            "publishes a new one. Choose a new location, or retire the existing table "
            "deliberately: see docs/engineering/events-activity-rebuild.md.")
    local = {part.name: part.stat().st_size for part in output.glob("*.parquet")}
    verify(output, days, files, users)

    # 1. The data, before any log exists, so the location is not yet a table.
    code, out, err = az("storage", "blob", "upload-batch", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-d", CONTAINER,
                        "--destination-path", REMOTE, "-s", str(output),
                        "--pattern", "*.parquet", "--overwrite", "--no-progress", "-o", "none")
    if code != 0:
        raise SystemExit(f"part upload failed, nothing published: {(err or out)[:500]}")
    print(f"uploaded {len(local)} part files")

    # 2. Every part at its full byte size before the log names them.
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

    # 3. The commit. This write is what makes the table readable.
    code, out, err = az("storage", "blob", "upload", "--account-name", ACCOUNT,
                        "--auth-mode", "login", "-c", CONTAINER,
                        "-n", f"{REMOTE}/_delta_log/{COMMIT}", "-f", str(log),
                        "--overwrite", "--no-progress", "-o", "none")
    if code != 0:
        raise SystemExit(f"the data is uploaded but the commit did not land: {(err or out)[:400]}")
    print(f"published {CONTAINER}/{REMOTE}")
    print(f"next: register it as {CATALOG}.{SCHEMA_NAME}.{TABLE_NAME} and re-point "
          "the dataset: docs/engineering/events-activity-rebuild.md")


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
        verify(args.output, args.days, args.files, args.users)
    elif args.step == "convert":
        convert(args.output, args.days, args.files, args.users)
    elif args.step == "upload":
        upload(args.output, args.days, args.files, args.users)
    return 0


if __name__ == "__main__":
    sys.exit(main())
