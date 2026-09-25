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
import time
from datetime import date, datetime
from decimal import Decimal
from typing import Any, Callable, Mapping

from fastapi import HTTPException

import database.metadata as db
from services import engine_system_store


FAMILY_TABLES: dict[str, tuple[str, ...]] = {
    "ai_configuration": ("ai_providers", "user_ai_keys"),
    "catalog_sources": ("catalog_sources",),
    "data_sources": ("data_sources",),
    "datasets": ("datasets",),
    "dataset_semantics": ("dataset_dimensions", "dataset_columns", "dataset_metrics"),
    "charts": ("charts",),
    "dashboards": ("dashboards",),
    "favorites": ("favorites",),
    "saved_queries": ("saved_queries",),
    "user_themes": ("user_themes",),
    "user_recents": ("user_recents",),
    "query_history": ("query_history",),
    "activity": ("activity",),
    "context_cache": ("context_snapshots", "context_answer_cache"),
    "dlm_generation": ("dlm_artifact", "dlm_value_index", "dlm_router", "dlm_answers", "dlm_sketch"),
    "chat_history": ("chat_sessions", "chat_messages"),
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


def _table_columns(rows: list[Mapping[str, Any]]) -> dict[str, str]:
    """Infer one stable Engine type per source column.

    PostgreSQL permits nulls and values whose runtime representation varies
    across rows. KaveonDB typed tables deliberately do not. A column remains
    scalar when every non-null value has the same type and is promoted to JSON
    when nullability or mixed values would otherwise make replay order matter.
    """
    observed: dict[str, set[str]] = {}
    for row in rows:
        for key, value in row.items():
            observed.setdefault(str(key), set()).add(_typed(value)["type"])
    result: dict[str, str] = {}
    for key, types in observed.items():
        non_null = types - {"null"}
        result[key] = next(iter(non_null)) if len(non_null) == 1 and "null" not in types else "json"
    return result


def _typed_for_column(value: Any, column_type: str) -> dict[str, Any]:
    if column_type == "json":
        return {"type": "json", "value": _json_value(value)}
    if value is None:
        return {"type": "json", "value": None}
    return _typed(value)


def _columns_match(target: Mapping[str, Any], expected: Mapping[str, Any]) -> bool:
    """Compare source columns while accounting for Engine ownership metadata."""
    actual = dict(target)
    if "owner_principal" not in expected:
        actual.pop("owner_principal", None)
    return actual == dict(expected)


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
    # Replay identity is derived from each row's stable key, so SQL row order
    # is irrelevant. Avoiding a global ORDER BY keeps large history tables
    # streaming-friendly and prevents an unnecessary sort before migration.
    result = query(f"SELECT * FROM {table}")
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
    read: Callable[..., Mapping[str, Any] | None] = engine_system_store.read_row,
    update: Callable[..., dict] = engine_system_store.update_row,
) -> dict[str, int | str]:
    """Replay one immutable source snapshot into the typed system table.

    The Engine mutation is idempotent at the migration layer: callers should
    compare the returned target row/revision and skip an already matching row
    before invoking this function on a resumed run.
    """
    rows = snapshot_table(table, query=query)
    column_types = _table_columns(rows)
    written = 0
    skipped = 0
    updated = 0
    # Pin one bounded target page up front. This avoids a round-trip GET for
    # every history row while retaining direct reads for post-commit retry
    # verification. Authority tables are capped below the page bound.
    target_cache: dict[str, dict | None] = {}
    # One bounded snapshot avoids reloading the ADLS manifest for every row.
    if rows and len(rows) <= 1000 and read is engine_system_store.read_row:
        try:
            page = engine_system_store.list_rows(table, actor, "Admin", limit=1000)
        except HTTPException as error:
            # A first replay has no typed table yet; the Engine reports that
            # as an invalid empty page. Other transport/auth failures remain
            # fatal and must not be mistaken for an empty target.
            if error.status_code != 502:
                raise
            page = {"rows": []}
        target_cache = {str(item["id"]): item for item in page.get("rows", [])
                        if isinstance(item, Mapping) and isinstance(item.get("id"), str)}
        known_target_ids = set(target_cache)
    else:
        known_target_ids = set()

    # Large authority tables (notably query_history) are migrated in bounded
    # pages.  Listing the target once lets retries remain idempotent without
    # issuing a GET for every source row.
    if rows and len(rows) > 1000 and read is engine_system_store.read_row:
        target_cache = {}
        cursor = None
        try:
            while True:
                page = engine_system_store.list_rows(table, actor, "Admin", limit=1000, cursor=cursor)
                for item in page.get("rows", []):
                    if isinstance(item, Mapping) and isinstance(item.get("id"), str):
                        target_cache[item["id"]] = item
                cursor = page.get("next_cursor")
                if not cursor:
                    break
        except HTTPException as error:
            if error.status_code != 502:
                raise
            target_cache = {}
        known_target_ids = set(target_cache)

    # One multi-value INSERT per bounded batch is materially faster than a
    # transaction and HTTP round-trip for every history row. Existing rows
    # remain on the normal CAS/update path below, so replay stays resumable.
    if rows and len(rows) > 1000 and write is engine_system_store.create_row:
        pending: list[tuple[str, dict[str, dict[str, Any]]]] = []
        for row in rows:
            record_id = _record_id(table, row)
            columns = {str(key): _typed_for_column(value, column_types[str(key)]) for key, value in row.items()}
            target = target_cache.get(record_id)
            if target is not None and _columns_match(target.get("columns", {}), columns):
                skipped += 1
                continue
            if target is not None:
                # Mismatched rows need the revision/owner-aware update path.
                continue
            pending.append((record_id, columns))
            if len(pending) == 100:
                try:
                    engine_system_store.create_rows(pending, actor, "Admin", table=table, owner_principal=actor)
                    written += len(pending)
                except Exception:
                    # A prior partial replay (or duplicate source key) can
                    # make a whole batch reject. Reconcile that bounded batch
                    # row-by-row before failing the migration; later batches
                    # retain the fast transaction path.
                    for row_id, row_columns in pending:
                        existing = read(table, row_id, actor, "Admin")
                        if existing is not None and _columns_match(existing.get("columns", {}), row_columns):
                            skipped += 1
                        elif existing is None:
                            write(table, row_id, row_columns, actor, "Admin", owner_principal=actor)
                            written += 1
                        else:
                            raise
                pending = []
        if pending:
            try:
                engine_system_store.create_rows(pending, actor, "Admin", table=table, owner_principal=actor)
                written += len(pending)
            except Exception:
                for row_id, row_columns in pending:
                    existing = read(table, row_id, actor, "Admin")
                    if existing is not None and _columns_match(existing.get("columns", {}), row_columns):
                        skipped += 1
                    elif existing is None:
                        write(table, row_id, row_columns, actor, "Admin", owner_principal=actor)
                        written += 1
                    else:
                        raise
        # Existing mismatches are rare; fall through to the regular loop for
        # those rows, while already matching/new rows are skipped safely.
        rows = [row for row in rows if _record_id(table, row) in target_cache and
                not _columns_match(target_cache[_record_id(table, row)].get("columns", {}),
                                   {str(key): _typed_for_column(value, column_types[str(key)]) for key, value in row.items()})]

    def initial_read(_table: str, row_id: str, _actor: str, _role: str):
        if row_id in known_target_ids:
            return target_cache[row_id]
        if rows and len(rows) <= 1000 and read is engine_system_store.read_row:
            return None
        return read(_table, row_id, _actor, _role)

    for row in rows:
        record_id = _record_id(table, row)
        columns = {str(key): _typed_for_column(value, column_types[str(key)]) for key, value in row.items()}
        target = initial_read(table, record_id, actor, "Admin")
        if target is not None and _columns_match(target.get("columns", {}), columns):
            skipped += 1
            continue
        if target is not None:
            revision = target.get("revision")
            if type(revision) is not int or revision < 1:
                raise RuntimeError(f"KaveonDB returned an invalid revision for {table}:{record_id}")
            # Preserve the target row's ownership identity when resuming a
            # migration.  Typed-row updates are owner guarded by the Engine;
            # rows created by an earlier replay/API writer must be updated as
            # that owner, while newly created rows use the migration actor.
            target_owner = target.get("columns", {}).get("owner_principal")
            row_actor = actor
            if isinstance(target_owner, Mapping) and target_owner.get("type") == "string":
                owner_value = target_owner.get("value")
                if isinstance(owner_value, str) and owner_value:
                    row_actor = owner_value
            last_error = None
            for attempt in range(6):
                try:
                    update(table, record_id, columns, revision, row_actor, "Admin")
                    last_error = None
                    break
                except Exception as error:
                    last_error = error
                    # A distributed commit can be durable even when the
                    # response is lost. Re-read before every retry so replay
                    # never duplicates a committed mutation or overwrites a
                    # newer revision. Short backoff also lets a transient
                    # storage/identity failure recover without hot-looping.
                    after = read(table, record_id, actor, "Admin")
                    if after is not None and _columns_match(after.get("columns", {}), columns):
                        last_error = None
                        break
                    retry_revision = after.get("revision") if after else None
                    if type(retry_revision) is not int or retry_revision < 1:
                        raise
                    revision = retry_revision
                    if attempt < 5:
                        time.sleep(min(2 ** attempt, 8))
            if last_error is not None:
                raise RuntimeError(f"replay update failed for {table}:{record_id}") from last_error
            updated += 1
            continue
        last_error = None
        for attempt in range(6):
            try:
                write(table, record_id, columns, actor, "Admin", owner_principal=actor)
                last_error = None
                break
            except Exception as error:
                last_error = error
                # A create can commit while its response is lost. Re-read
                # after every uncertain outcome; retry only while absent.
                after = read(table, record_id, actor, "Admin")
                if after is not None and _columns_match(after.get("columns", {}), columns):
                    last_error = None
                    break
                if after is not None:
                    raise
                if attempt < 5:
                    time.sleep(min(2 ** attempt, 8))
        if last_error is not None:
            raise RuntimeError(f"replay create failed for {table}:{record_id}") from last_error
        written += 1
    return {"family": _TABLE_TO_FAMILY[table], "table": table, "source_count": len(rows),
            "written": written, "updated": updated, "skipped": skipped}


def replay_family(family: str, actor: str = "kaveon-migration", **kwargs: Any) -> dict[str, Any]:
    if family not in FAMILY_TABLES:
        raise ValueError("unsupported system authority family")
    reports = [replay_table(table, actor, **kwargs) for table in FAMILY_TABLES[family]]
    return {"family": family, "tables": reports, "source_count": sum(int(r["source_count"]) for r in reports),
            "written": sum(int(r["written"]) for r in reports)}
