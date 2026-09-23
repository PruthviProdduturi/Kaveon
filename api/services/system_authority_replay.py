"""Lossless replay of PostgreSQL control-plane families into KaveonDB.

These families are not product documents: they are typed system rows.  This
module is the single migration boundary for them.  It deliberately takes a
snapshot from PostgreSQL, normalizes values deterministically, and writes
through ``engine_system_store`` so a replay can be resumed safely by the
retirement runner without teaching each router about transport details.
"""

from __future__ import annotations

import hashlib
import json
from datetime import date, datetime
from decimal import Decimal
from typing import Any, Callable, Mapping

import database.metadata as db
from services import engine_system_store


FAMILY_TABLES: dict[str, tuple[str, ...]] = {
    "ai_configuration": ("ai_providers", "user_ai_keys"),
    "catalog_sources": ("catalog_sources",),
    "data_sources": ("data_sources",),
    "dataset_semantics": ("dataset_dimensions", "dataset_columns", "dataset_metrics"),
    "context_cache": ("context_snapshots", "context_answer_cache"),
    "dlm_generation": ("dlm_artifact", "dlm_value_index", "dlm_router", "dlm_answers", "dlm_sketch"),
}

# The table names are static and reviewed; never accept a caller-provided SQL
# identifier here.
_TABLE_TO_FAMILY = {table: family for family, tables in FAMILY_TABLES.items() for table in tables}


def _json_value(value: Any) -> Any:
    """Convert driver values to deterministic JSON-compatible values."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, Decimal):
        return str(value)
    if isinstance(value, (bytes, bytearray, memoryview)):
        return bytes(value).hex()
    if isinstance(value, Mapping):
        return {str(key): _json_value(child) for key, child in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_value(child) for child in value]
    # JSONB drivers may return a custom mapping-like value; fail closed rather
    # than silently losing metadata during retirement.
    raise TypeError(f"unsupported metadata value type: {type(value).__name__}")


def _typed(value: Any) -> dict[str, Any]:
    value = _json_value(value)
    if value is None:
        return {"type": "null", "value": None}
    if isinstance(value, bool):
        return {"type": "boolean", "value": value}
    if isinstance(value, int) and not isinstance(value, bool):
        return {"type": "integer", "value": value}
    if isinstance(value, (dict, list)):
        return {"type": "json", "value": value}
    return {"type": "string", "value": str(value)}


def _record_id(table: str, row: Mapping[str, Any]) -> str:
    """Return a stable <=255 character key for a control-plane row."""
    for key in ("id", "dataset_id", "user_email"):
        value = row.get(key)
        if value is not None and str(value):
            candidate = f"{table}:{value}"
            if len(candidate) <= 255:
                return candidate
    canonical = json.dumps(_json_value(dict(row)), sort_keys=True, separators=(",", ":"))
    return f"{table}:sha256:{hashlib.sha256(canonical.encode()).hexdigest()}"


def snapshot_table(table: str, *, query: Callable[..., Mapping[str, Any]] = db.query) -> list[dict[str, Any]]:
    if table not in _TABLE_TO_FAMILY:
        raise ValueError("unsupported system authority table")
    result = query(f"SELECT * FROM {table} ORDER BY 1")
    rows = result.get("rows") if isinstance(result, Mapping) else None
    if not isinstance(rows, list):
        raise RuntimeError(f"PostgreSQL returned an invalid {table} snapshot")
    return [dict(row) for row in rows]


def replay_table(
    table: str,
    actor: str = "kaveon-migration",
    *,
    query: Callable[..., Mapping[str, Any]] = db.query,
    write: Callable[..., dict] = engine_system_store.create_row,
) -> dict[str, int | str]:
    """Replay one immutable source snapshot into the typed system table.

    The Engine mutation is idempotent at the migration layer: callers should
    compare the returned target row/revision and skip an already matching row
    before invoking this function on a resumed run.
    """
    rows = snapshot_table(table, query=query)
    written = 0
    for row in rows:
        record_id = _record_id(table, row)
        columns = {str(key): _typed(value) for key, value in row.items()}
        write(table, record_id, columns, actor, "Admin", owner_principal=actor)
        written += 1
    return {"family": _TABLE_TO_FAMILY[table], "table": table, "source_count": len(rows), "written": written}


def replay_family(family: str, actor: str = "kaveon-migration", **kwargs: Any) -> dict[str, Any]:
    if family not in FAMILY_TABLES:
        raise ValueError("unsupported system authority family")
    reports = [replay_table(table, actor, **kwargs) for table in FAMILY_TABLES[family]]
    return {"family": family, "tables": reports, "source_count": sum(int(r["source_count"]) for r in reports),
            "written": sum(int(r["written"]) for r in reports)}

