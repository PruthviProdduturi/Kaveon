"""Bootstrap the local Docker showcase catalog from the ADLS mirror.

The catalog database is durable, but it is intentionally separate from the lake
files.  A fresh Docker volume therefore needs the definitions recreated.  This
script discovers ``data/adls-mirror/opensource/showcase/<schema>/<table>`` and
registers each table through the Engine's native, idempotent catalog DDL.

It never writes lake data and it refuses paths outside the showcase root.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def sql_ident(value: str, label: str) -> str:
    if not IDENTIFIER.fullmatch(value):
        raise ValueError(f"unsafe {label}: {value!r}")
    return value


def sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


class Engine:
    def __init__(self, endpoint: str, token: str, timeout: float) -> None:
        self.endpoint = endpoint.rstrip("/")
        self.token = token
        self.timeout = timeout

    def statement(self, sql: str) -> dict:
        request = Request(
            self.endpoint + "/v1/statement",
            data=sql.encode("utf-8"),
            method="POST",
            headers={
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "text/plain; charset=utf-8",
                "X-Trino-User": "local-bootstrap",
                "X-Kaveon-Actor": "local-bootstrap",
                "X-Kaveon-Role": "admin",
            },
        )
        try:
            with urlopen(request, timeout=self.timeout) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except (HTTPError, URLError) as error:
            body = error.read().decode("utf-8", "replace") if isinstance(error, HTTPError) else str(error)
            raise RuntimeError(f"Engine statement failed: {body[:500]}") from error
        while payload.get("nextUri"):
            with urlopen(
                Request(
                    self.endpoint + payload["nextUri"] if payload["nextUri"].startswith("/") else payload["nextUri"],
                    headers={"Authorization": f"Bearer {self.token}", "X-Trino-User": "local-bootstrap"},
                ),
                timeout=self.timeout,
            ) as response:
                payload = json.loads(response.read().decode("utf-8"))
        if payload.get("state") == "FAILED" or payload.get("error"):
            raise RuntimeError(f"Engine rejected {sql}: {json.dumps(payload.get('error', payload))[:800]}")
        return payload

    def table_exists(self, catalog: str, schema: str, table: str) -> bool:
        try:
            self.statement(f"SHOW CREATE TABLE {catalog}.{schema}.{table}")
        except RuntimeError as error:
            if "does not exist" in str(error).lower() or "not found" in str(error).lower():
                return False
            raise
        return True


def discover(root: Path) -> list[tuple[str, str, str]]:
    root = root.resolve()
    if not root.is_dir():
        raise RuntimeError(f"showcase root does not exist: {root}")
    tables: list[tuple[str, str, str]] = []
    for schema_dir in sorted(root.iterdir()):
        if not schema_dir.is_dir() or not IDENTIFIER.fullmatch(schema_dir.name):
            continue
        for table_dir in sorted(schema_dir.iterdir()):
            if not table_dir.is_dir() or not IDENTIFIER.fullmatch(table_dir.name):
                continue
            if (table_dir / "_delta_log").is_dir():
                fmt = "delta"
            elif any(table_dir.glob("*.parquet")):
                fmt = "parquet"
            else:
                continue
            tables.append((schema_dir.name, table_dir.name, fmt))
    if not tables:
        raise RuntimeError(f"no readable Delta or Parquet tables under {root}")
    return tables


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", default=os.getenv("KAVEON_ENGINE_URL", "http://127.0.0.1:8081"))
    parser.add_argument("--token", default=os.getenv("KAVEON_ENGINE_ADMIN_TOKEN", "kaveon-local-admin-token-not-for-production"))
    parser.add_argument("--root", type=Path, default=Path("data/adls-mirror/opensource/showcase"),
                        help="host directory containing showcase schema/table directories")
    parser.add_argument("--base-path", default=os.getenv("KAVEON_LOCAL_CATALOG_BASE_PATH", "/data/showcase"),
                        help="the same showcase root as seen inside the Engine container")
    parser.add_argument("--catalog", default="OpenSource")
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()
    catalog = sql_ident(args.catalog, "catalog")
    tables = discover(args.root)
    engine = Engine(args.endpoint, args.token, args.timeout)
    if not args.base_path.startswith("/") or ".." in Path(args.base_path).parts:
        raise ValueError("--base-path must be an absolute container path without '..'")
    engine.statement(f"CREATE CATALOG IF NOT EXISTS {catalog} WITH (storage = 'local', base_path = {sql_string(args.base_path)})")
    for schema, table, fmt in tables:
        schema = sql_ident(schema, "schema")
        table = sql_ident(table, "table")
        engine.statement(f"CREATE SCHEMA IF NOT EXISTS {catalog}.{schema}")
        if engine.table_exists(catalog, schema, table):
            print(f"kept {catalog}.{schema}.{table} (already registered)", flush=True)
            continue
        location = f"{schema}/{table}"
        engine.statement(
            f"CALL {catalog}.system.register_table(schema_name => {sql_string(schema)}, "
            f"table_name => {sql_string(table)}, table_location => {sql_string(location)}, "
            f"format => {sql_string(fmt)})"
        )
        print(f"registered {catalog}.{schema}.{table} ({fmt})", flush=True)
    print(json.dumps({"catalog": catalog, "tables": len(tables), "root": str(args.root.resolve())}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, ValueError) as error:
        print(f"bootstrap-local-catalog: {error}", file=sys.stderr)
        raise SystemExit(1)
