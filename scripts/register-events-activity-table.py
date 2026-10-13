"""Register `KaveonTelemetry.events.daily_user_activity` and declare its cube shape.

This is step 2 of the rebuild migration in
`docs/engineering/events-activity-rebuild.md`. The catalog, the schema and the
container already exist — `register-kaveon-telemetry-catalog.py` created them —
so this adds **one table** beside the ones already there and touches nothing
else. In particular it never looks at, re-registers, or removes
`KaveonTelemetry.events.enriched`, which is what the installation is still
reading while this runs.

    kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000

    python scripts/register-events-activity-table.py                  # reads only
    python scripts/register-events-activity-table.py --apply
    python scripts/register-events-activity-table.py --apply --shape

Nothing is written without `--apply`.

## Why the expected row count comes from the manifest

`register-kaveon-telemetry-catalog.py` holds `row_count: 504_000_000` as a
literal, which was correct exactly once. The rebuilt table's row count is a
consequence of its presence draws, so it is read from the `build-manifest.json`
the build wrote next to the part files, and the registration probe is compared
against that. A mismatch means the lake does not hold what was built, which is
the one thing worth failing on here.

## What this deliberately does not do

**It does not run `ANALYZE`.** Statistics and the cube are a cost decision: the
cube build needs the host resized up and back (see
`docs/engineering/lake-delta-and-cube.md`), and it runs for a long time. This
script declares the shape so the cube *can* be built, and prints the call that
builds it.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_kaveon_events_table import (  # noqa: E402
    CATALOG, DEFAULT_OUTPUT, METRICS, SCHEMA_NAME, TABLE_NAME, USER_DIMENSIONS)

BASE = os.environ.get("KAVEON_PORTAL_URL", "http://127.0.0.1:13015").rstrip("/")

# The table's location inside the catalog's root path, which is `events` — so
# the objects live at `telemetry/events/daily_user_activity`. Same shape as the two tables
# already registered: the location is the directory name, nothing more.
LOCATION = TABLE_NAME

# The cube axes and measures, taken from the generator's own column lists so a
# column added there cannot be silently left off the cube. `surface` leads
# because it is the one dimension that is a property of the row rather than of
# the user.
SHAPE_DIMENSIONS = ("surface", *USER_DIMENSIONS)
# Mirrors the declaration in docs/engineering/lake-delta-and-cube.md:
# `latency_p75_ms` keeps `count` beside `sum` because the charts average it and
# a weighted average needs both, and `user_id` is a measure rather than an axis
# because it is only ever counted distinct — which the cube keeps as a
# HyperLogLog sketch, so a 36-character text key costs no more there than an
# integer did.
SHAPE_MEASURES = {
    "actions": "sum,count",
    "latency_p75_ms": "sum,count,min,max",
}


def call(method: str, path: str, body=None, *, expect=(200, 201)):
    """One API call through the Studio proxy, which stamps the identity."""
    data = json.dumps(body).encode("utf-8") if body is not None else None
    request = urllib.request.Request(
        f"{BASE}/api/kaveon{path}", data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {},
    )
    try:
        with urllib.request.urlopen(request, timeout=600) as response:
            payload = response.read().decode("utf-8")
            if response.status not in expect:
                raise SystemExit(f"{method} {path} -> HTTP {response.status}: {payload[:400]}")
            return json.loads(payload) if payload else None
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace")[:400]
        if error.code in expect:
            return json.loads(detail) if detail else None
        raise SystemExit(f"{method} {path} -> HTTP {error.code}: {detail}") from None
    except urllib.error.URLError as error:
        raise SystemExit(
            f"cannot reach {BASE}: {error.reason}\n"
            "Port-forward the portal first:\n"
            "  kubectl --context kaveon-aks -n kaveon port-forward "
            "svc/kaveon-portal 13015:3000"
        ) from None


def statement(query: str):
    """Run one statement through the Engine and return its rows."""
    result = call("POST", "/api/v1/sql/engine",
                  {"query": query, "catalog": CATALOG, "schema": SCHEMA_NAME,
                   "source": "catalog-curation"})
    if isinstance(result, dict) and result.get("error"):
        raise SystemExit(f"{query!r} failed: {result['error']}")
    return (result or {}).get("data") or (result or {}).get("rows") or []


def expected_rows(manifest_path: Path) -> tuple[int, dict]:
    if not manifest_path.is_file():
        raise SystemExit(
            f"no build manifest at {manifest_path}.\n"
            "It is written beside the part files by "
            "`build_kaveon_events_table.py build`. Point --manifest at the "
            "build that was uploaded, so the probe is compared against the "
            "table that was actually published rather than against a guess.")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("table") != f"{CATALOG}.{SCHEMA_NAME}.{TABLE_NAME}":
        raise SystemExit(f"{manifest_path} describes {manifest.get('table')!r}, "
                         f"not {CATALOG}.{SCHEMA_NAME}.{TABLE_NAME}")
    return int(manifest["row_count"]), manifest


def catalog_definition_id() -> str:
    definitions = call("GET", "/api/v1/engine/catalog/definitions") or []
    if isinstance(definitions, dict):
        definitions = definitions.get("definitions") or []
    for definition in definitions:
        if isinstance(definition, dict) and definition.get("name") == CATALOG:
            return str(definition.get("id"))
    raise SystemExit(
        f"the Engine has no {CATALOG} definition. Register the catalog first "
        "with scripts/register-kaveon-telemetry-catalog.py.")


def schema_id_for(catalog_id: str) -> str:
    """The schema definition id. `POST …/definitions/{id}/schemas` defaults it
    to `{catalog_id}-{name}`, which is the form the catalog registration used,
    but it is read back from the Engine rather than assumed."""
    schemas = call("GET", "/api/v1/engine/catalog/definitions/"
                          f"{quote(catalog_id, safe='')}/schemas") or []
    if isinstance(schemas, dict):
        schemas = schemas.get("schemas") or []
    for schema in schemas:
        if isinstance(schema, dict) and schema.get("name") == SCHEMA_NAME:
            return str(schema.get("id"))
    raise SystemExit(f"{CATALOG} has no {SCHEMA_NAME} schema definition; "
                     "register the catalog first.")


def registered_tables(schema_id: str) -> dict[str, dict]:
    tables = call("GET", "/api/v1/engine/catalog/schemas/"
                         f"{quote(schema_id, safe='')}/tables") or []
    if isinstance(tables, dict):
        tables = tables.get("tables") or []
    return {str(table.get("name")): table for table in tables
            if isinstance(table, dict)}


def shape_statement() -> str:
    dimensions = ", ".join(f"'{name}'" for name in SHAPE_DIMENSIONS)
    measures = ", ".join(
        f"'{name}:{SHAPE_MEASURES.get(name, 'sum')}'" for name in METRICS)
    return (f"ALTER TABLE {SCHEMA_NAME}.{TABLE_NAME} SET SHAPE (\n"
            f"    dimensions = ARRAY[{dimensions}],\n"
            f"    measures   = ARRAY[{measures}, 'user_id:count_distinct']\n"
            f")")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--apply", action="store_true",
                        help="register the table; without it nothing is written")
    parser.add_argument("--shape", action="store_true",
                        help="with --apply: also declare the cube shape")
    parser.add_argument("--manifest", type=Path,
                        default=DEFAULT_OUTPUT / "build-manifest.json",
                        help="the build-manifest.json of the build that was uploaded")
    args = parser.parse_args()

    rows, manifest = expected_rows(args.manifest)
    print(f"Portal: {BASE}")
    print(f"Target: {CATALOG}.{SCHEMA_NAME}.{TABLE_NAME} -> {LOCATION}")
    print(f"Manifest: {rows:,} rows, {manifest.get('distinct_users'):,} distinct "
          f"users, {manifest.get('file_count')} files, seed {manifest.get('seed')}")

    catalog_id = catalog_definition_id()
    schema_id = schema_id_for(catalog_id)
    existing = registered_tables(schema_id)
    print(f"Already registered in {CATALOG}: {sorted(existing) or 'nothing'}")
    if TABLE_NAME in existing:
        raise SystemExit(
            f"{CATALOG}.{SCHEMA_NAME}.{TABLE_NAME} is already registered. "
            "This script only ever adds the table; re-registering it is not "
            "something to do by accident.")

    if not args.apply:
        print("\nPreflight only. Nothing was written.")
        print("With --apply this would register the table, verify it by probe, "
              f"and compare the probed count against {rows:,}.")
        print("\nThe shape it would declare with --shape:\n")
        print(shape_statement())
        return 0

    created = call("POST", "/api/v1/engine/catalog/tables",
                   {"schema_id": schema_id, "name": TABLE_NAME,
                    "location": LOCATION, "format": "Delta",
                    "access": "Shortcut", "verify": True}) or {}
    probe = created.get("probe") or {}
    counted = probe.get("rowCount")
    columns = len((created.get("table") or {}).get("columns") or [])
    print(f"Registered {CATALOG}.{SCHEMA_NAME}.{TABLE_NAME} "
          f"({columns} columns read from the Delta log)")
    if counted is not None and int(counted) != rows:
        raise SystemExit(
            f"the registration probe counted {int(counted):,} rows where the "
            f"manifest says {rows:,}. The table is registered against storage "
            "that is not the build this manifest describes. Resolve that before "
            "pointing anything at it.")
    print(f"Probe counted {int(counted):,} rows, matching the manifest"
          if counted is not None else "Probe returned no row count")

    confirmed = statement(f"SELECT COUNT(*) FROM {SCHEMA_NAME}.{TABLE_NAME}")
    if not confirmed or int(confirmed[0][0]) != rows:
        found = confirmed[0][0] if confirmed else "nothing"
        raise SystemExit(f"COUNT(*) through the catalog returned {found}, "
                         f"expected {rows:,}")
    print(f"COUNT(*) through the catalog agrees: {rows:,}")

    if args.shape:
        statement(shape_statement())
        print("Declared the cube shape")

    print(f"\n{CATALOG}.{SCHEMA_NAME}.{TABLE_NAME} is registered and readable.")
    print("Nothing points at it yet. Next, in order:")
    print("  1. build statistics and the cube, off the request path, with the "
          "host resized up:")
    print(f'       engine_bridge.analyze_table("{CATALOG}", "{SCHEMA_NAME}", '
          f'"{TABLE_NAME}", actor, "Admin",')
    print("                                   sketches=True, distinct=True, "
          "cube=True, timeout=10_800)")
    print("  2. re-point the dataset: "
          "python scripts/register-kaveon-events-dataset.py --apply --generate")
    print("  3. verify the charts, then retire events.enriched.")
    print("See docs/engineering/events-activity-rebuild.md.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
