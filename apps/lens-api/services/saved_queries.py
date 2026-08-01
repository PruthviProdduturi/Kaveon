"""Saved queries service — port of savedQueries.service.ts."""

from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db


_SELECT = """
    SELECT id, name, description, sql_text, dataset_id, tables_used, run_context,
           parameters, row_limit, last_run_at, last_run_status, last_run_row_count,
           last_run_duration_ms, created_by, created_at, modified_by, modified_at,
           is_shared, tags, chart_id, dashboard_id, favorite
    FROM saved_queries
"""


def _adapt(row: dict) -> dict:
    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "sql": row.get("sql_text"),
        "created_at": row.get("created_at"),
        "updated_at": row.get("modified_at"),
        "created_by": row.get("created_by"),
    }


def list_saved_queries(user_id: str) -> List[dict]:
    result = db.query(
        _SELECT + "WHERE created_by = @param0 ORDER BY modified_at DESC",
        [user_id],
    )
    return [_adapt(r) for r in result["rows"]]


def get_by_id(query_id: str, user_id: str) -> Optional[dict]:
    row = db.query_one(
        _SELECT + "WHERE id = @param0 AND created_by = @param1",
        [int(query_id), user_id],
    )
    return _adapt(row) if row else None


def create_saved_query(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    db.execute("""
        INSERT INTO saved_queries (name, description, sql_text, created_by, created_at,
                                   modified_by, modified_at, is_shared, favorite)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8)
    """, [
        data["name"], data.get("description"), data["sql"],
        user_id, now, user_id, now, False, False,
    ])
    inserted = db.query_one(
        "SELECT TOP 1 id FROM saved_queries WHERE name = @param0 AND created_by = @param1 ORDER BY id DESC",
        [data["name"], user_id],
    )
    if not inserted:
        raise RuntimeError("Failed to retrieve created saved query")
    created = get_by_id(str(inserted["id"]), user_id)
    if not created:
        raise RuntimeError("Failed to retrieve created saved query")
    return created


def update_saved_query(query_id: str, data: dict, user_id: str) -> Optional[dict]:
    if not get_by_id(query_id, user_id):
        return None

    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0

    if "name" in data:
        updates.append(f"name = @param{i}"); params.append(data["name"]); i += 1
    if "description" in data:
        updates.append(f"description = @param{i}"); params.append(data["description"]); i += 1
    if "sql" in data:
        updates.append(f"sql_text = @param{i}"); params.append(data["sql"]); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    updates.append(f"modified_by = @param{i}"); params.append(user_id); i += 1

    params.append(int(query_id))
    params.append(user_id)

    db.execute(
        f"UPDATE saved_queries SET {', '.join(updates)} "
        f"WHERE id = @param{i} AND created_by = @param{i + 1}",
        params,
    )
    return get_by_id(query_id, user_id)


def delete_saved_query(query_id: str, user_id: str) -> bool:
    count = db.execute(
        "DELETE FROM saved_queries WHERE id = @param0 AND created_by = @param1",
        [int(query_id), user_id],
    )
    return count > 0
