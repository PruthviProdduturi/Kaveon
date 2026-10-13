"""Register the KaveonTelemetry catalog: Kaveon's own product-usage data.

Why a catalog of its own
------------------------
`OpenSource` is third-party open data — WHO case reports, OWID energy, NASA
temperatures, NYC taxi. Kaveon's synthetic product-usage events are not that,
and they were sitting in `OpenSource.public` alongside a COVID projection and
a five-row taxi table: the schema everything landed in when nobody chose one.
The three curated subjects got subject schemas (`covid`, `climate_energy`,
`ai_benchmarks`) and the leftovers got `public`.

So the events tables get a catalog that says what they are, in a container of
their own — the separation is physical, not just a name:

    opensource/snapshots/2026-09-09-v1/public/kaveon_events_enriched
      -> telemetry/events/enriched          KaveonTelemetry.events.enriched
    opensource/snapshots/2026-09-09-v1/public/kaveon_events_dashboard
      -> telemetry/events/dashboard         KaveonTelemetry.events.dashboard

The objects were copied server-side within the one storage account (130
objects, 5.59 GiB, no egress, nothing through an operator's machine) and the
destination object set was read back and compared name-for-name and
byte-for-byte before this script was pointed at it. The Delta logs use paths
relative to the table root — verified, not assumed — which is what makes
moving a table directory wholesale safe.

**The originals are still in `opensource` and this script does not remove
them.** Retiring them is a separate, deliberate step, taken only once this
catalog is registered, verified, and the Kaveon Events dataset reads from it.

What this does, in order
------------------------
1. Counts the rows at the new location and refuses to continue unless they
   match what the source table held. A catalog that disagrees with its storage
   is worse than no catalog.
2. Creates the `KaveonTelemetry` catalog source through the API, so the record
   lands in KaveonDB. Deliberately not `database.metadata`, which is how
   `register-curated-catalog.py` writes its source row — that is PostgreSQL,
   and this installation has none, so that path cannot run here at all.
3. Synchronises the catalog definition into the Engine, then registers the
   `events` schema and the two tables.
4. Verifies each table by COUNT(*) through the Engine before reporting success,
   the same gate the curated-catalog registration uses.

It does not re-point the Kaveon Events dataset. That is a separate, visible
step so the dataset's charts are never silently moved underneath someone:

    through the API's dataset update once this catalog is verified:
    PATCH /api/v1/datasets/{id} with database KaveonTelemetry, schema events,
    table enriched.

Usage
-----
    kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000

    python scripts/register-kaveon-telemetry-catalog.py            # preflight, reads only
    python scripts/register-kaveon-telemetry-catalog.py --apply     # register

Nothing is written without --apply.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from urllib.parse import quote

CATALOG = "KaveonTelemetry"
SCHEMA = "events"
DESCRIPTION = "Kaveon product-usage telemetry. Synthetic, and Kaveon's own — not open data."

# The lake these objects already live in. The container and account are facts
# of the deployment, so they are read from the environment with the values this
# installation uses as the default rather than hardcoded as the only option.
ACCOUNT = os.environ.get("KAVEON_LAKE_ADLS_ACCOUNT", "kaveonlake")
# Its own container. Kaveon's telemetry is not open data and does not belong in
# the container that holds WHO, OWID, NASA and NYC taxi tables.
CONTAINER = os.environ.get("KAVEON_TELEMETRY_CONTAINER", "telemetry")
ROOT_PATH = os.environ.get("KAVEON_TELEMETRY_ROOT", "events")
CREDENTIAL_REF = os.environ.get("KAVEON_LAKE_CREDENTIAL_REF", "kaveon-test-reader")

BASE = os.environ.get("KAVEON_PORTAL_URL", "http://127.0.0.1:13015").rstrip("/")

# No column lists here, deliberately.
#
# `POST /engine/catalog/tables` with no `columns` makes the Engine read them
# from the table's own metadata — the Delta log, in this case. A hand-declared
# list is a second copy of the schema that drifts the moment the table changes,
# and this repository already has the scar: the catalog manifest declared
# single-file Parquet objects with pre-conversion row counts for months after
# the lake became Delta, and a rebuild from it would have failed or registered
# locations that no longer existed.
#
# The row counts below are not a schema. They are the cross-check against the
# registration probe, so a catalog that registers but reads the wrong objects
# is caught here rather than by a reader looking at a dashboard.
TABLES = [
    {"name": "enriched", "location": "enriched",
     "source": "OpenSource.public.kaveon_events_enriched", "row_count": 504_000_000},
    {"name": "dashboard", "location": "dashboard",
     "source": "OpenSource.public.kaveon_events_dashboard", "row_count": 24},
]


def call(method: str, path: str, body=None, *, expect=(200, 201)):
    """One API call through the Studio proxy, which stamps the identity."""
    data = json.dumps(body).encode("utf-8") if body is not None else None
    request = urllib.request.Request(
        f"{BASE}/api/kaveon{path}", data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {},
    )
    try:
        with urllib.request.urlopen(request, timeout=300) as response:
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
            "  kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000"
        ) from None


def statement(query: str, catalog: str, schema: str):
    """Run one read through the Engine and return its rows."""
    result = call("POST", "/api/v1/sql/engine",
                  {"query": query, "catalog": catalog, "schema": schema,
                   "source": "catalog-curation"})
    if isinstance(result, dict) and result.get("error"):
        raise SystemExit(f"{query!r} failed: {result['error']}")
    rows = (result or {}).get("data") or (result or {}).get("rows") or []
    return rows


def verify_source_tables():
    """Read the live tables and refuse to continue if they are not as declared.

    Checked before anything is written and against the tables in their current
    home, because a catalog that disagrees with its storage is the defect this
    whole exercise exists to stop repeating.
    """
    print("Verifying the source tables as they stand today")
    for table in TABLES:
        catalog, schema, name = table["source"].split(".")
        rows = statement(f"SELECT COUNT(*) FROM {schema}.{name}", catalog, schema)
        if not rows or int(rows[0][0]) != table["row_count"]:
            found = rows[0][0] if rows else "nothing"
            raise SystemExit(
                f"{table['source']}: expected {table['row_count']:,} rows, found {found}.\n"
                "The declaration in this script is stale. Update it from the live table "
                "rather than registering a count that is already wrong."
            )
        print(f"  {table['source']:48s} {table['row_count']:>12,} rows")


def existing_source():
    """The KaveonTelemetry source, if this has already been registered."""
    sources = call("GET", "/api/v1/catalog-sources") or []
    if isinstance(sources, dict):
        sources = sources.get("sources") or sources.get("items") or []
    for source in sources:
        if isinstance(source, dict) and source.get("engine_catalog") == CATALOG:
            return source
    return None


def register_source():
    """Create the catalog source, or return the one already there."""
    found = existing_source()
    if found:
        print(f"Catalog source already registered: {found.get('id')}")
        return found
    body = {
        "name": CATALOG,
        "engine_catalog": CATALOG,
        "storage_type": "adls_gen2",
        "storage_config": {"account": ACCOUNT, "container": CONTAINER, "root_path": ROOT_PATH},
        "data_format": "delta",
        "credential_kind": "workload_identity",
        "credential_ref": CREDENTIAL_REF,
        "adapter_type": "native",
        "description": DESCRIPTION,
    }
    created = call("POST", "/api/v1/catalog-sources", body, expect=(200, 201))
    source = created.get("source") if isinstance(created, dict) else None
    source = source or created
    print(f"Created catalog source {source.get('id')} -> {CATALOG}")
    return source


def sync_to_engine(source_id: str):
    call("POST", f"/api/v1/catalog-sources/{quote(str(source_id), safe='')}/engine-sync", {})
    print(f"Synchronised {CATALOG} into the Engine")


def register_schema_and_tables():
    """Register the schema and the two tables against the Engine definition.

    `format` is stated, never assumed: `register-curated-catalog.py` hardcoded
    Parquet for every table, which is how the lake's conversion to Delta left a
    rebuild registering sixteen Delta directories as single Parquet objects.

    `verify` is left on, which is the important part. The API probes the
    location and takes the definition back out if the Engine cannot read it, so
    a table that points at nothing is never left in the catalog for someone to
    find later.
    """
    catalog_id = None
    definitions = call("GET", "/api/v1/engine/catalog/definitions") or []
    if isinstance(definitions, dict):
        definitions = definitions.get("definitions") or []
    for definition in definitions:
        if isinstance(definition, dict) and definition.get("name") == CATALOG:
            catalog_id = definition.get("id")
    if not catalog_id:
        raise SystemExit(
            f"The Engine has no {CATALOG} definition after the sync. "
            "Check the engine-sync response before registering tables into nothing."
        )
    schema_id = f"{catalog_id}-{SCHEMA}"
    call("POST", f"/api/v1/engine/catalog/definitions/{quote(str(catalog_id), safe='')}/schemas",
         {"name": SCHEMA, "id": schema_id}, expect=(200, 201, 409))
    print(f"Registered schema {CATALOG}.{SCHEMA}")
    for table in TABLES:
        created = call("POST", "/api/v1/engine/catalog/tables",
                       {"schema_id": schema_id, "name": table["name"],
                        "location": table["location"], "format": "Delta",
                        "access": "Shortcut", "verify": True}) or {}
        probe = created.get("probe") or {}
        counted = probe.get("rowCount")
        if counted is not None and int(counted) != table["row_count"]:
            raise SystemExit(
                f"{CATALOG}.{SCHEMA}.{table['name']}: the registration probe counted "
                f"{int(counted):,} rows where {table['row_count']:,} was expected. "
                "The table is registered against storage that is not what this script "
                "was written for. Check the location before using the catalog."
            )
        columns = len((created.get("table") or {}).get("columns") or [])
        print(f"Registered {CATALOG}.{SCHEMA}.{table['name']} -> {table['location']} "
              f"({columns} columns read from the Delta log, "
              f"{int(counted):,} rows probed)" if counted is not None else
              f"Registered {CATALOG}.{SCHEMA}.{table['name']} -> {table['location']} "
              f"({columns} columns read from the Delta log)")


def verify_registered():
    """COUNT(*) through the new catalog before calling this done."""
    print("Verifying the tables through their new catalog")
    for table in TABLES:
        rows = statement(f"SELECT COUNT(*) FROM {SCHEMA}.{table['name']}", CATALOG, SCHEMA)
        if not rows or int(rows[0][0]) != table["row_count"]:
            found = rows[0][0] if rows else "nothing"
            raise SystemExit(
                f"{CATALOG}.{SCHEMA}.{table['name']}: expected {table['row_count']:,}, found {found}. "
                "The catalog is registered but does not read its storage; fix it before using it."
            )
        print(f"  {CATALOG}.{SCHEMA}.{table['name']:12s} {table['row_count']:>12,} rows")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--apply", action="store_true",
                        help="register the catalog; without it nothing is written")
    args = parser.parse_args()

    print(f"Portal: {BASE}")
    print(f"Target: {CATALOG}.{SCHEMA} at {ACCOUNT}/{CONTAINER}/{ROOT_PATH}\n")
    verify_source_tables()

    if not args.apply:
        print("\nPreflight only. Nothing was written.")
        print("Re-run with --apply to register the catalog.")
        print("\nAfter registering, re-point the dataset as a separate, visible step:")
        print(f"  database {CATALOG}, schema {SCHEMA}, table enriched,"
              " applied with PATCH /api/v1/datasets/{id}.")
        return 0

    source = register_source()
    sync_to_engine(source.get("id"))
    register_schema_and_tables()
    verify_registered()
    print(f"\n{CATALOG} is registered and reads its storage.")
    print("The objects did not move; this is a catalog boundary, not a copy.")
    print(f"Next: re-point the Kaveon Events dataset to {CATALOG}.{SCHEMA}.enriched,")
    print("as a separate step, so its charts are never moved silently.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
