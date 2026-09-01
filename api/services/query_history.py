"""Query history service — persists SQL Lab / dataset query runs.

Columns match the live `query_history` table exactly. T-SQL idioms
(TOP / OUTPUT INSERTED) are translated per-dialect by database.metadata.
"""

import uuid
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db


_COLS = (
    "id, sql_text, database_name, executed_at, execution_time, row_count, "
    "status, error_message, user_email, trigger_source, dataset_id, tables_used"
)


def list_history(user_id: Optional[str], limit: int = 50) -> List[dict]:
    fetch_all = not user_id or user_id == "all"
    if fetch_all:
        result = db.query(
            f"SELECT TOP (@param0) {_COLS} FROM query_history ORDER BY executed_at DESC",
            [limit],
        )
    else:
        result = db.query(
            f"SELECT TOP (@param1) {_COLS} FROM query_history "
            f"WHERE user_email = @param0 ORDER BY executed_at DESC",
            [user_id, limit],
        )
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
    }


def delete_all_history(user_id: str) -> int:
    return db.execute(
        "DELETE FROM query_history WHERE user_email = @param0",
        [user_id],
    )
