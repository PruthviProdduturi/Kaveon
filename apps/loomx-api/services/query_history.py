"""Query history service — port of queryHistory.service.ts."""

from datetime import datetime
from typing import List, Optional
import database.metadata as db


_SELECT = """
    SELECT id, query_id, dataset_id, tables_used, sql_text, run_context,
           trigger_source, status, error_message, row_count, duration_ms,
           started_at, finished_at, executed_by, client_ip, user_agent
    FROM query_history
"""


def list_history(user_id: Optional[str], limit: int = 50) -> List[dict]:
    fetch_all = not user_id or user_id == "all"
    if fetch_all:
        result = db.query(
            f"SELECT TOP (@param0) {_SELECT.split('FROM')[1].strip()} FROM query_history ORDER BY started_at DESC",
            [limit],
        )
    else:
        result = db.query(
            _SELECT + "WHERE executed_by = @param0 ORDER BY started_at DESC",
            [user_id],
        )
        result["rows"] = result["rows"][:limit]
    return result["rows"]


def create_history(data: dict, user_id: str) -> dict:
    now = datetime.utcnow()
    started_at = data.get("started_at")
    if isinstance(started_at, (int, float)):
        started_at = datetime.utcfromtimestamp(started_at / 1000)
    elif started_at is None:
        started_at = now

    duration_ms = data.get("duration_ms") or 0
    finished_at = datetime.utcfromtimestamp(
        started_at.timestamp() + duration_ms / 1000
    ) if started_at else now

    trigger_source = data.get("trigger_source") or "lab"

    result = db.query("""
        INSERT INTO query_history (
            query_id, dataset_id, tables_used, sql_text, run_context,
            trigger_source, status, error_message, row_count, duration_ms,
            started_at, finished_at, executed_by, client_ip, user_agent
        ) OUTPUT INSERTED.id VALUES (
            @param0, @param1, @param2, @param3, @param4,
            @param5, @param6, @param7, @param8, @param9,
            @param10, @param11, @param12, @param13, @param14
        )
    """, [
        data.get("query_id"),
        data.get("dataset_id"),
        data.get("tables_used"),
        data["sql_text"],
        data.get("run_context"),
        trigger_source,
        data["status"],
        data.get("error_message"),
        data.get("row_count"),
        duration_ms,
        started_at,
        finished_at,
        user_id,
        data.get("client_ip"),
        data.get("user_agent"),
    ])

    created_id = result["rows"][0]["id"] if result["rows"] else None

    return {
        "id": created_id,
        "sql_text": data["sql_text"],
        "status": data["status"],
        "trigger_source": trigger_source,
        "started_at": started_at,
        "finished_at": finished_at,
        "executed_by": user_id,
        "duration_ms": duration_ms,
        "row_count": data.get("row_count"),
    }


def delete_all_history(user_id: str) -> int:
    return db.execute(
        "DELETE FROM query_history WHERE executed_by = @param0",
        [user_id],
    )
