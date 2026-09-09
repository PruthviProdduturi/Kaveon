"""Query history service — persists SQL Lab / dataset query runs.

Columns match the live `query_history` table exactly. T-SQL idioms
(TOP / OUTPUT INSERTED) are translated per-dialect by database.metadata.
"""

import uuid
import json
import threading
import time
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db


_BASE_COLS = (
    "id, sql_text, database_name, executed_at, execution_time, row_count, "
    "status, error_message, user_email, trigger_source, dataset_id, tables_used"
)
_ENGINE_COLS = ", engine_query_id, engine_details"
_SCHEMA_CACHE_TTL_SECONDS = 60.0
_schema_cache: tuple[float, bool] | None = None
_schema_lock = threading.Lock()


def _supports_engine_details() -> bool:
    """Capability-detect additive telemetry columns for rolling upgrades."""
    global _schema_cache
    now = time.monotonic()
    with _schema_lock:
        if _schema_cache and now - _schema_cache[0] < _SCHEMA_CACHE_TTL_SECONDS:
            return _schema_cache[1]
        try:
            rows = db.query(
                "SELECT column_name FROM information_schema.columns WHERE table_name = @param0",
                ["query_history"],
            )["rows"]
        except Exception:
            _schema_cache = (now, False)
            return False
        columns = {str(row.get("column_name", "")).casefold() for row in rows}
        supported = {"engine_query_id", "engine_details"} <= columns
        _schema_cache = (now, supported)
        return supported


def _engine_metadata(data: dict) -> tuple[Optional[str], Optional[str]]:
    details = data.get("engine_details")
    if not isinstance(details, dict):
        return data.get("engine_query_id"), None
    safe = {
        key: details.get(key)
        for key in (
            "rows_are_preview", "scan_metrics_complete", "submitted_at_ms",
            "completed_at_ms", "timings", "scans", "stages", "context",
        )
        if key in details
    }
    return data.get("engine_query_id") or details.get("id"), json.dumps(safe, separators=(",", ":"))


def list_history(user_id: Optional[str], limit: int = 50) -> List[dict]:
    columns = _BASE_COLS + (_ENGINE_COLS if _supports_engine_details() else "")
    fetch_all = not user_id or user_id == "all"
    if fetch_all:
        result = db.query(
            f"SELECT TOP (@param0) {columns} FROM query_history ORDER BY executed_at DESC",
            [limit],
        )
    else:
        result = db.query(
            f"SELECT TOP (@param1) {columns} FROM query_history "
            f"WHERE user_email = @param0 ORDER BY executed_at DESC",
            [user_id, limit],
        )
    for row in result["rows"]:
        raw = row.get("engine_details")
        if isinstance(raw, str):
            try:
                row["engine_details"] = json.loads(raw)
            except (TypeError, ValueError):
                row["engine_details"] = None
    return result["rows"]


def create_history(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    started_at = data.get("started_at")
    if isinstance(started_at, (int, float)):
        started_at = datetime.fromtimestamp(started_at / 1000, tz=timezone.utc).replace(tzinfo=None)
    elif started_at is None:
        started_at = now

    execution_time = data.get("duration_ms") or 0
    trigger_source = data.get("trigger_source") or "lab"
    # id is a varchar with no DB default (like dashboards) — generate app-side.
    new_id = str(uuid.uuid4())

    engine_query_id, engine_details = _engine_metadata(data)
    if _supports_engine_details():
        db.query("""
        INSERT INTO query_history (
            id, sql_text, database_name, executed_at, execution_time, row_count,
            status, error_message, user_email, trigger_source, dataset_id, tables_used,
            engine_query_id, engine_details
        ) VALUES (
            @param0, @param1, @param2, @param3, @param4, @param5,
            @param6, @param7, @param8, @param9, @param10, @param11, @param12, @param13
        )
    """, [
        new_id, data["sql_text"], data.get("database_name"), started_at,
        execution_time, data.get("row_count"), data["status"], data.get("error_message"),
        user_id, trigger_source, data.get("dataset_id"), data.get("tables_used"),
        engine_query_id, engine_details,
    ])
    else:
        db.query("""
        INSERT INTO query_history (
            id, sql_text, database_name, executed_at, execution_time, row_count,
            status, error_message, user_email, trigger_source, dataset_id, tables_used
        ) VALUES (
            @param0, @param1, @param2, @param3, @param4, @param5,
            @param6, @param7, @param8, @param9, @param10, @param11
        )
        """, [
        new_id,
        data["sql_text"],
        data.get("database_name"),
        started_at,
        execution_time,
        data.get("row_count"),
        data["status"],
        data.get("error_message"),
        user_id,
        trigger_source,
        data.get("dataset_id"),
        data.get("tables_used"),
        ])

    return {
        "id": new_id,
        "sql_text": data["sql_text"],
        "database_name": data.get("database_name"),
        "status": data["status"],
        "trigger_source": trigger_source,
        "executed_at": started_at,
        "user_email": user_id,
        "execution_time": execution_time,
        "row_count": data.get("row_count"),
        "engine_query_id": engine_query_id,
        "engine_details": json.loads(engine_details) if engine_details else None,
    }


def delete_all_history(user_id: str) -> int:
    return db.execute(
        "DELETE FROM query_history WHERE user_email = @param0",
        [user_id],
    )
