"""Register a lake catalog, its schemas and its tables with a Kaveon API.

    python scripts/register-lake-catalog.py \
        --api https://kaveon-api.example.com \
        --catalog OpenSource \
        --account mylake --container opensource --root snapshots/2026-09-09-v1 \
        --schema public:covid_global,nyc_taxi_borough \
        --schema ai_benchmarks:arena_battles,leaderboard,pricing

Columns are deliberately not supplied. The Engine reads them from the table's
own metadata — the Delta log, the Iceberg metadata, or the first Parquet
footer — and `verify` makes it read the location once before the table joins
the catalog, so an unreadable path registers nothing rather than a table that
fails on its first query.

Re-running is safe: anything already registered is reported and skipped.

Identity, not secrets: the deployment's own managed or workload identity must
already have read access to the container. This registers a location; it does
not grant access to one.
"""
import argparse
import json
import os
import sys
import urllib.error
import urllib.request


def call(base, headers, method, path, body=None, timeout=300):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read()
            return response.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as error:
        payload = error.read().decode(errors="replace")
        try:
            return error.code, json.loads(payload)
        except ValueError:
            return error.code, {"detail": payload[:400]}
    except Exception as error:  # transport failure
        return 0, {"detail": repr(error)[:300]}


def listed(body, *keys):
    """The catalog endpoints answer either a bare list or an object wrapping one."""
    if isinstance(body, list):
        return [item for item in body if isinstance(item, dict)]
    if isinstance(body, dict):
        for key in keys:
            value = body.get(key)
            if isinstance(value, list):
                return [item for item in value if isinstance(item, dict)]
    return []


def main():
    options = argparse.ArgumentParser(description=__doc__,
                                      formatter_class=argparse.RawDescriptionHelpFormatter)
    options.add_argument("--api", required=True, help="Base URL of the Kaveon API")
    options.add_argument("--catalog", required=True)
    options.add_argument("--account", required=True, help="ADLS Gen2 storage account")
    options.add_argument("--container", required=True)
    options.add_argument("--root", default="", help="Path within the container every table is relative to")
    options.add_argument("--schema", action="append", required=True, metavar="NAME:t1,t2",
                         help="A schema and its tables; repeatable")
    options.add_argument("--format", default="parquet", choices=["parquet", "delta", "iceberg"])
    options.add_argument("--actor", default=os.getenv("KAVEON_ACTOR", ""),
                         help="Administrator identity the registration is attributed to")
    args = options.parse_args()

    secret = os.getenv("KAVEON_PROXY_SECRET", "")
    if not secret or not args.actor:
        sys.exit("set KAVEON_PROXY_SECRET and --actor (or KAVEON_ACTOR); "
                 "this script registers through the authenticated API, not the Engine directly")

    base = args.api.rstrip("/") + "/api/v1"
    headers = {"Content-Type": "application/json", "x-proxy-secret": secret,
               "x-user-email": args.actor, "x-user-role": "Admin"}

    wanted = {}
    for entry in args.schema:
        name, _, tables = entry.partition(":")
        wanted[name.strip()] = [t.strip() for t in tables.split(",") if t.strip()]

    status, body = call(base, headers, "GET", "/engine/catalog/definitions")
    catalogs = {c.get("name"): c.get("id") for c in listed(body, "definitions", "catalogs")}
    catalog_id = catalogs.get(args.catalog)
    if catalog_id:
        print(f"catalog {args.catalog} already defined ({catalog_id})")
    else:
        status, body = call(base, headers, "POST", "/engine/catalog/definitions", {
            "name": args.catalog,
            "storage": {"type": "adls_gen2", "account": args.account,
                        "container": args.container, "root_path": args.root},
            "format": args.format,
        })
        if status not in (200, 201):
            sys.exit(f"catalog registration failed ({status}): {json.dumps(body)[:300]}")
        catalog_id = (body.get("catalog") or body).get("id") if isinstance(body, dict) else None
        print(f"catalog {args.catalog} registered ({catalog_id})")

    status, body = call(base, headers, "GET", f"/engine/catalog/definitions/{catalog_id}/schemas")
    known = {s.get("name"): s.get("id") for s in listed(body, "schemas", "definitions")}

    registered = present = failed = 0
    for schema, tables in wanted.items():
        schema_id = known.get(schema)
        if not schema_id:
            status, body = call(base, headers, "POST",
                                f"/engine/catalog/definitions/{catalog_id}/schemas",
                                {"catalog_id": catalog_id, "name": schema})
            schema_id = (body.get("schema") or body).get("id") if isinstance(body, dict) else None
            if not schema_id:
                print(f"  schema {schema}: not created ({status}); skipping its tables")
                failed += len(tables)
                continue
            print(f"  schema {schema} registered")
        for table in tables:
            status, body = call(base, headers, "POST", "/engine/catalog/tables", {
                "schema_id": schema_id, "name": table,
                "location": f"{schema}/{table}", "format": args.format,
                "columns": [], "verify": True,
            })
            rendered = json.dumps(body)
            if status in (200, 201):
                registered += 1
                probe = body.get("probe") if isinstance(body, dict) else None
                rows = probe.get("rowCount") if isinstance(probe, dict) else None
                print(f"    {schema}.{table}: registered"
                      + (f" ({rows:,} rows verified)" if isinstance(rows, int) else ""))
            elif status == 409 or "exists" in rendered.lower():
                present += 1
                print(f"    {schema}.{table}: already registered")
            else:
                failed += 1
                print(f"    {schema}.{table}: FAILED {status} {rendered[:160]}")

    print(f"\nregistered {registered}, already present {present}, failed {failed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
