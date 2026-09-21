"""Write an Iceberg v2 table beside its Parquet twin, for qualifying the
Engine's Iceberg reader against the Parquet reader over identical rows.

    python scripts/make-iceberg-fixture.py <lake root> [--rows 200000]

Writes <root>/qualification/iceberg_twin/ (an Iceberg table: metadata/,
data/, written by pyiceberg with a SQLite catalog kept in the same
directory) and <root>/qualification/parquet_twin.parquet (the same rows).
Three commits: an initial append, a second append, and a delete of one
region, so the snapshot the Engine pins carries added and removed files.
Deterministic (seeded), so a rerun reproduces the same rows.
"""
import argparse
import datetime as dt
import pathlib
import random

import pyarrow as pa
import pyarrow.parquet as pq
from pyiceberg.catalog.sql import SqlCatalog


def rows(count, seed):
    rng = random.Random(seed)
    regions = ["eu", "us", "apac", "latam"]
    platforms = ["web", "mobile", "desktop"]
    base = dt.date(2026, 1, 1)
    ids, region, platform, day, amount, active = [], [], [], [], [], []
    for i in range(count):
        ids.append(seed * 10_000_000 + i)
        region.append(rng.choice(regions))
        platform.append(rng.choice(platforms))
        day.append(base + dt.timedelta(days=rng.randrange(120)))
        amount.append(round(rng.uniform(0, 500), 2) if rng.random() > 0.05 else None)
        active.append(rng.random() > 0.3)
    return pa.table({
        "id": pa.array(ids, pa.int64()),
        "region": pa.array(region, pa.string()),
        "platform": pa.array(platform, pa.string()),
        "day": pa.array(day, pa.date32()),
        "amount": pa.array(amount, pa.float64()),
        "active": pa.array(active, pa.bool_()),
    })


def main():
    args = argparse.ArgumentParser()
    args.add_argument("root")
    args.add_argument("--rows", type=int, default=200_000)
    args.add_argument("--as", dest="as_root", default=None,
                      help="the path the readers will see the lake root at (e.g. /data); "
                           "recorded in the table's metadata instead of the host path")
    options = args.parse_args()
    root = pathlib.Path(options.root).resolve() / "qualification"
    root.mkdir(parents=True, exist_ok=True)
    warehouse = root / "iceberg_twin"
    if warehouse.exists():
        raise SystemExit(f"{warehouse} exists; remove it to regenerate")
    warehouse.mkdir()
    # A plain path, not a file:// URI: pyiceberg's local IO turns a Windows
    # drive URI into a path that is not one.
    # The metadata records absolute paths; when the readers mount the lake
    # elsewhere (the Compose stack sees it at /data), write the table under
    # that path — pyiceberg's local IO resolves it here through the
    # environment's cwd-independent absolute path, so the run creates a
    # junction/symlink at that path pointing at the host directory.
    seen_root = pathlib.Path(options.as_root) if options.as_root else pathlib.Path(options.root).resolve()
    seen_warehouse = (seen_root / "qualification" / "iceberg_twin").as_posix()
    catalog = SqlCatalog("fixture", uri=f"sqlite:///{(warehouse / 'catalog.db').as_posix()}",
                         warehouse=seen_warehouse)
    catalog.create_namespace("q")
    first, second = rows(options.rows, 1), rows(options.rows // 4, 2)
    table = catalog.create_table("q.twin", schema=first.schema, location=seen_warehouse)
    table.append(first)
    table.append(second)
    table.delete("region = 'latam'")
    table = catalog.load_table("q.twin")
    kept = table.scan().to_arrow()
    kept = kept.sort_by("id")
    pq.write_table(kept, root / "parquet_twin.parquet", row_group_size=50_000)
    print({"iceberg": warehouse.as_posix(), "metadata": table.metadata_location,
           "snapshot_id": table.current_snapshot().snapshot_id, "rows": kept.num_rows,
           "parquet": (root / "parquet_twin.parquet").as_posix()})


if __name__ == "__main__":
    main()
