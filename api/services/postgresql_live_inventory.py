"""Read-only live PostgreSQL authority-table discovery for retirement evidence."""

import hashlib
import json
from datetime import datetime, timezone

import database.metadata as db
from services.postgresql_retirement_gate import AUTHORITY_FAMILIES

SCHEMA_VERSION = 1
MAX_TABLES = 256
INFRASTRUCTURE_TABLES = frozenset({"product_migration_outbox"})


def _canonical(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def collect(*, now=None) -> dict:
    """Discover public base tables and count maintained authorities exactly."""
    expected = {
        table: family
        for family, tables in AUTHORITY_FAMILIES.items()
        for table in tables
    }
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        snapshot = transaction.query_one("SELECT txid_current_snapshot() AS snapshot") or {}
        discovered_rows = transaction.query("""
            SELECT table_name
            FROM information_schema.tables
            WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
            ORDER BY table_name
            LIMIT @param0
        """, [MAX_TABLES + 1])["rows"]
        if len(discovered_rows) > MAX_TABLES:
            raise RuntimeError("PostgreSQL live schema exceeds its table bound")
        discovered = tuple(str(row["table_name"]) for row in discovered_rows)
        if len(discovered) != len(set(discovered)) or discovered != tuple(sorted(discovered)):
            raise RuntimeError("PostgreSQL live schema inventory is not stable")
        unknown = sorted(set(discovered) - set(expected) - set(INFRASTRUCTURE_TABLES))
        if unknown:
            raise RuntimeError("unclassified PostgreSQL public tables: " + ", ".join(unknown))
        counts = {}
        for table in sorted(set(discovered) & set(expected)):
            # The identifier comes only from the fixed authority manifest.
            row = transaction.query_one(f'SELECT COUNT(*) AS count FROM "{table}"') or {}
            count = row.get("count")
            if type(count) is not int or count < 0:
                raise RuntimeError(f"PostgreSQL table count is invalid: {table}")
            counts[table] = count

    captured_at = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    report = {
        "schema_version": SCHEMA_VERSION,
        "captured_at": captured_at.isoformat().replace("+00:00", "Z"),
        "source_snapshot": str(snapshot.get("snapshot") or ""),
        "discovered_tables": list(discovered),
        "authority_tables": [
            {"table": table, "family": expected[table], "present": table in discovered,
             "row_count": counts.get(table, 0)}
            for table in sorted(expected)
        ],
        "infrastructure_tables": sorted(set(discovered) & set(INFRASTRUCTURE_TABLES)),
        "unclassified_tables": [],
    }
    if not report["source_snapshot"]:
        raise RuntimeError("PostgreSQL source snapshot identity is missing")
    report["report_sha256"] = hashlib.sha256(_canonical(report)).hexdigest()
    return report
