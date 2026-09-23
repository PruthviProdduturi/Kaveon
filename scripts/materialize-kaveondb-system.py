"""Materialize the local read-only KaveonDB.system projection.

The files are immutable snapshots of the KaveonDB transaction authority.  The
same command is safe to rerun after a product commit; the coordinator is
restarted so its catalog publication observes the new snapshot.
"""
from __future__ import annotations

import argparse
import json
import urllib.parse
import urllib.request
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

FAMILIES = {
    "datasets": "dataset", "charts": "chart", "dashboards": "dashboard",
    "saved_queries": "saved_query", "dlm_definitions": "dlm_definition",
    "dlm_runs": "dlm_run", "query_history": "query_history", "sources": "source",
    "favorites": "favorite", "user_themes": "user_theme", "user_recents": "user_recent",
    "activity": "activity", "chat_sessions": "chat_session", "chat_messages": "chat_message",
}

def get_json(base: str, path: str, token: str) -> object:
    request = urllib.request.Request(
        urllib.parse.urljoin(base.rstrip("/") + "/", path.lstrip("/")),
        headers={"Authorization": f"Bearer {token}"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)

def rows(records: list[dict]) -> dict[str, list]:
    return {
        "id": [str(item.get("id", "")) for item in records],
        "revision": [int(item.get("revision", 0)) for item in records],
        "generation": [int(item.get("generation", 0)) for item in records],
        "snapshot_id": [str(item.get("snapshot_id", "")) for item in records],
        "document_json": [json.dumps(item.get("document", {}), sort_keys=True, separators=(",", ":")) for item in records],
    }

def write_table(root: Path, name: str, records: list[dict]) -> int:
    target = root / f"{name}.parquet"
    target.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(pa.table(rows(records)), target, compression="zstd")
    return len(records)

def product_records(base: str, kind: str, token: str) -> list[dict]:
    records: list[dict] = []
    cursor: str | None = None
    while True:
        suffix = "?limit=100" + (("&cursor=" + urllib.parse.quote(cursor, safe="")) if cursor else "")
        payload = get_json(base, f"/v1/products/{kind}{suffix}", token)
        records.extend(payload.get("records", []))
        cursor = payload.get("next_cursor")
        if not cursor:
            return records

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", default="http://localhost:8081")
    parser.add_argument("--token", default="kaveon-local-admin-token-not-for-production")
    parser.add_argument("--root", type=Path, default=Path("data/adls-mirror/opensource/kaveon/system"))
    args = parser.parse_args()
    for table, kind in FAMILIES.items():
        count = write_table(args.root, table, product_records(args.engine, kind, args.token))
        print(f"{table}: {count}")
    # Catalog/security metadata is intentionally represented as JSON documents;
    # the source authority remains the Engine catalog and access ledger.
    for table, path in {
        "catalogs": "/v1/catalog/definitions",
        "schemas": "/v1/catalog/definitions",
        "tables": "/v1/catalog/definitions",
        "permissions": "/v1/admin/catalog-access",
        "audit_log": "/v1/audit?limit=1000",
    }.items():
        payload = get_json(args.engine, path, args.token)
        records = payload if isinstance(payload, list) else payload.get("grants", payload.get("records", [payload]))
        if not isinstance(records, list): records = [records]
        wrapped = [{"id": str(i), "revision": 1, "generation": 0, "snapshot_id": "", "document": row} for i, row in enumerate(records)]
        print(f"{table}: {write_table(args.root, table, wrapped)}")

if __name__ == "__main__":
    main()
