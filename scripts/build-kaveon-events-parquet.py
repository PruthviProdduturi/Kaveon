"""Build the 504M-row product telemetry table for KaveonDB.

Reproduces `scripts/build_504m.py` exactly — the same hash-derived columns for
the same 3,000,000 users over 2026-07-04..2026-07-31 and six surfaces — but
writes one sorted Parquet file instead of loading PostgreSQL. Each user's
dimensions are joined in at build time, so the Engine serves the table the
legacy `kaveon_events_enriched` view used to compute on every query.

One step, no database, idempotent:

    python scripts/build-kaveon-events-parquet.py build --output tmp/kaveon-events

Users are drawn deterministically from the same weighted pools
`data/kaveon-usage/generate_usage.py` used (the legacy warehouse assigned them
with `random()`, so nothing depends on that particular draw). The build writes:

    <output>/kaveon_product/kaveon_events_users/combined-v1.parquet
    <output>/kaveon_product/kaveon_events_enriched/combined-v1.parquet
    <output>/kaveon-events-singlefile-manifest.json

The manifest is the shape `scripts/register-curated-catalog.py` consumes. Row
groups are one (day, surface) chunk each, written in date order, so the
Engine's row-group statistics prune date filters without a directory layout.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

N_USERS = 3_000_000
FIRST_DAY = 4                    # 2026-07-04
DAYS = 28
SEED = 20260704
DISCLAIMER = "Deterministic demo telemetry; not production data."

# (code, name, actions, sessions, duration_sec, queries_run, charts_created,
#  errors, rows_scanned, cache_hits, latency_p75_ms) — identical to build_504m.
SURFACES = [
    (1, "Chat",          (3, 15),  (1, 4), (60, 1800),   (0, 3),  (0, 0), (0, 1), (0, 1000),       (0, 2),  (100, 500)),
    (2, "Dashboard",     (5, 25),  (1, 5), (120, 3600),  (2, 10), (0, 1), (0, 2), (1000, 100000),   (2, 8),  (200, 1500)),
    (3, "Chart Builder", (3, 12),  (1, 3), (180, 2400),  (3, 15), (1, 5), (0, 2), (5000, 500000),   (1, 5),  (300, 2000)),
    (4, "SQL Lab",       (5, 20),  (1, 4), (300, 3600),  (5, 25), (0, 1), (0, 3), (10000, 1000000), (0, 3),  (500, 3000)),
    (5, "API",           (10, 50), (1, 2), (30, 600),    (1, 5),  (0, 0), (0, 1), (100, 50000),     (3, 10), (50, 500)),
    (6, "Export",        (1, 5),   (1, 2), (30, 300),    (1, 3),  (0, 0), (0, 1), (50000, 1000000), (0, 2),  (200, 1000)),
]

USER_DIMS = ("platform", "license", "segment", "industry", "region", "country",
             "deployment", "acquisition_channel", "team_size")
METRICS = ("actions", "sessions", "duration_sec", "queries_run", "charts_created",
           "errors", "rows_scanned", "cache_hits", "latency_p75_ms")

UIDS = np.arange(1, N_USERS + 1, dtype=np.int64)


def hcol(seed: int, lo: int, hi: int) -> np.ndarray:
    rng = hi - lo + 1
    if rng <= 0:
        return np.full(N_USERS, lo, dtype=np.int64)
    return lo + np.abs((UIDS * seed) % rng)


def chunk_metrics(day_off: int, sc: int, ranges) -> dict:
    act_r, sess_r, dur_r, qr_r, ch_r, err_r, rs_r, cache_r, lat_r = ranges
    return {
        "actions":        hcol(486187 + sc * 31 + day_off * 127 + 1, *act_r),
        "sessions":       hcol(999331 + sc * 53 + day_off * 193 + 2, *sess_r),
        "duration_sec":   hcol(1300813 + sc * 997 + day_off * 251 + 5, *dur_r),
        "queries_run":    hcol(735391 + sc * 71 + day_off * 311 + 3, *qr_r),
        "charts_created": hcol(571373 + sc * 97 + day_off * 409 + 4, *ch_r),
        "errors":         hcol(412619 + sc * 113 + day_off * 503 + 6, *err_r),
        "rows_scanned":   hcol(2654435761 + sc * 40503 + day_off * 12289 + 7, *rs_r),
        "cache_hits":     hcol(297179 + sc * 137 + day_off * 601 + 8, *cache_r),
        "latency_p75_ms": hcol(193939 + sc * 151 + day_off * 701 + 9, *lat_r),
    }


def manifest_columns(schema: pa.Schema) -> list:
    out = []
    for f in schema:
        if pa.types.is_floating(f.type):
            t = "Float64"
        elif pa.types.is_integer(f.type):
            t = "Int64"
        else:
            t = "Utf8"
        out.append({"name": f.name, "data_type": t, "nullable": True})
    return out


# ── users ───────────────────────────────────────────────────────────────────
# The weighted pools from data/kaveon-usage/generate_usage.py; duplicates
# raise a value's share. Geography maps locale -> country -> region.

GEO = {
    "en-US": ("United States", "North America"), "en-CA": ("Canada", "North America"),
    "es-MX": ("Mexico", "North America"), "en-GB": ("United Kingdom", "Europe"),
    "de-DE": ("Germany", "Europe"), "fr-FR": ("France", "Europe"), "nl-NL": ("Netherlands", "Europe"),
    "es-ES": ("Spain", "Europe"), "sv-SE": ("Sweden", "Europe"), "it-IT": ("Italy", "Europe"),
    "hi-IN": ("India", "Asia"), "ja-JP": ("Japan", "Asia"), "en-SG": ("Singapore", "Asia"),
    "ko-KR": ("South Korea", "Asia"), "id-ID": ("Indonesia", "Asia"), "zh-CN": ("China", "Asia"),
    "pt-BR": ("Brazil", "South America"), "es-AR": ("Argentina", "South America"),
    "es-CO": ("Colombia", "South America"), "es-CL": ("Chile", "South America"),
    "en-NG": ("Nigeria", "Africa"), "en-ZA": ("South Africa", "Africa"), "sw-KE": ("Kenya", "Africa"),
    "ar-EG": ("Egypt", "Africa"), "en-AU": ("Australia", "Oceania"), "en-NZ": ("New Zealand", "Oceania"),
}
PLATFORM = {"desktop-win": "Desktop", "desktop-mac": "Desktop", "desktop-linux": "Desktop",
            "mobile-ios": "Mobile", "mobile-android": "Mobile", "web-win": "Web",
            "web-mac": "Web", "web-linux": "Web", "web-chromeos": "Web"}
POOLS = {
    "locale": ("en-US,en-US,en-US,en-US,en-CA,es-MX,en-GB,en-GB,de-DE,fr-FR,nl-NL,es-ES,sv-SE,it-IT,"
               "hi-IN,hi-IN,ja-JP,en-SG,ko-KR,id-ID,zh-CN,pt-BR,es-AR,es-CO,es-CL,"
               "en-NG,en-ZA,sw-KE,ar-EG,en-AU,en-NZ").split(","),
    "platform_key": ("desktop-win,desktop-win,desktop-win,desktop-mac,desktop-mac,desktop-linux,"
                     "mobile-ios,mobile-ios,mobile-android,mobile-android,mobile-android,"
                     "web-win,web-win,web-mac,web-linux,web-chromeos").split(","),
    "license": "Free,Free,Free,Standard,Standard,Professional,Professional,Enterprise".split(","),
    "segment": "Enterprise,Enterprise,Mid-Market,Mid-Market,Mid-Market,SMB,SMB,Startup,Startup".split(","),
    "industry": ("Technology,Technology,Technology,Healthcare,Financial Services,Manufacturing,"
                 "Retail,Education,Media,Energy,Government,Logistics,Real Estate,Professional Services").split(","),
    "team_size": "Solo,Solo,Small,Small,Small,Medium,Medium,Large,Enterprise".split(","),
    "deployment": "Cloud,Cloud,Cloud,Cloud,Hybrid,On-Premise".split(","),
    "acquisition_channel": "Organic,Organic,Organic,Referral,Referral,Partner,Paid,Paid,Direct".split(","),
}


def build_users(output: Path) -> pa.Table:
    """user_id 1..N with one deterministic draw per attribute (a fixed-seed
    generator per pool, so adding a pool never reshuffles another)."""
    def draw(name: str, pool: list) -> np.ndarray:
        rng = np.random.default_rng(SEED + sum(ord(c) for c in name))
        return rng.integers(0, len(pool), size=N_USERS, dtype=np.int32)

    def column(name: str, indices: np.ndarray, values: list) -> pa.DictionaryArray:
        return pa.DictionaryArray.from_arrays(pa.array(indices), pa.array(values))

    locale_idx = draw("locale", POOLS["locale"])
    columns = {
        "user_id": pa.array(UIDS),
        "platform": column("platform", draw("platform_key", POOLS["platform_key"]), [PLATFORM[k] for k in POOLS["platform_key"]]),
        "license": column("license", draw("license", POOLS["license"]), POOLS["license"]),
        "segment": column("segment", draw("segment", POOLS["segment"]), POOLS["segment"]),
        "industry": column("industry", draw("industry", POOLS["industry"]), POOLS["industry"]),
        "region": column("region", locale_idx, [GEO[k][1] for k in POOLS["locale"]]),
        "country": column("country", locale_idx, [GEO[k][0] for k in POOLS["locale"]]),
        "deployment": column("deployment", draw("deployment", POOLS["deployment"]), POOLS["deployment"]),
        "acquisition_channel": column("acquisition_channel", draw("acquisition_channel", POOLS["acquisition_channel"]), POOLS["acquisition_channel"]),
        "team_size": column("team_size", draw("team_size", POOLS["team_size"]), POOLS["team_size"]),
        "locale": column("locale", locale_idx, POOLS["locale"]),
    }
    users = pa.table(columns)
    dest = output / "kaveon_product" / "kaveon_events_users" / "combined-v1.parquet"
    dest.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(users, dest, compression="zstd", row_group_size=N_USERS)
    return users


# ── build ───────────────────────────────────────────────────────────────────

def build(output: Path) -> None:
    users = build_users(output)
    # Each dimension is already dictionary-typed; chunks reuse dictionary + indices.
    dims = {}
    for d in USER_DIMS:
        arr = users.column(d).combine_chunks()
        dims[d] = (arr.dictionary, arr.indices.to_numpy(zero_copy_only=False).astype(np.int32))

    # Low-cardinality text is dictionary-typed in memory; Parquet stores it as
    # plain UTF-8 with dictionary encoding, which is what the Engine reads.
    text = pa.dictionary(pa.int32(), pa.string())
    fields = [("event_date", text), ("user_id", pa.int64()), ("surface", text)]
    fields += [(m, pa.int64()) for m in METRICS]
    fields += [(d, text) for d in USER_DIMS]
    schema = pa.schema(fields)

    dest = output / "kaveon_product" / "kaveon_events_enriched" / "combined-v1.parquet"
    dest.parent.mkdir(parents=True, exist_ok=True)
    writer = pq.ParquetWriter(dest, schema, compression="zstd", use_dictionary=True,
                              write_statistics=True)
    total = 0
    t0 = time.time()
    surface_names = {sc: name for sc, name, *_ in SURFACES}
    for day_off in range(DAYS):
        day = f"2026-07-{FIRST_DAY + day_off:02d}"
        for sc, name, *ranges in SURFACES:
            m = chunk_metrics(day_off, sc, ranges)
            cols = {
                "event_date": pa.DictionaryArray.from_arrays(np.zeros(N_USERS, dtype=np.int32), pa.array([day])),
                "user_id": pa.array(UIDS),
                "surface": pa.DictionaryArray.from_arrays(np.zeros(N_USERS, dtype=np.int32), pa.array([surface_names[sc]])),
            }
            for metric in METRICS:
                cols[metric] = pa.array(m[metric])
            for d in USER_DIMS:
                dictionary, indices = dims[d]
                cols[d] = pa.DictionaryArray.from_arrays(pa.array(indices), dictionary)
            table = pa.table(cols)
            # One row group per (day, surface): statistics bound event_date and surface exactly.
            writer.write_table(table, row_group_size=N_USERS)
            total += N_USERS
        elapsed = time.time() - t0
        print(f"{day}: {total / 1e6:.0f}M rows, {elapsed / 60:.1f} min, "
              f"ETA {(elapsed / (day_off + 1)) * (DAYS - day_off - 1) / 60:.0f} min", flush=True)
    writer.close()

    manifest = {
        "synthetic": True,
        "disclaimer": DISCLAIMER,
        "seed": SEED,
        "tables": [
            {"schema": "kaveon_product", "name": "kaveon_events_users",
             "location": "kaveon_product/kaveon_events_users/combined-v1.parquet",
             "row_count": N_USERS, "columns": manifest_columns(users.schema)},
            {"schema": "kaveon_product", "name": "kaveon_events_enriched",
             "location": "kaveon_product/kaveon_events_enriched/combined-v1.parquet",
             "row_count": total, "columns": manifest_columns(schema)},
        ],
    }
    (output / "kaveon-events-singlefile-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"done: {total:,} rows, {dest.stat().st_size / 1e9:.2f} GB", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("step", choices=["build"])
    parser.add_argument("--output", type=Path, default=Path("tmp/kaveon-events"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    build(args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
