"""Charts service — port of charts.service.ts."""

import json
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db

VALID_VISIBILITY = {"private", "internal", "published"}


def _adapt(row: dict) -> dict:
    query_config, viz_config, config = {}, {}, {}
    try:
        query_config = json.loads(row.get("query_config") or "{}")
        viz_config = json.loads(row.get("viz_config") or "{}")
        config = {**query_config, **viz_config}
    except Exception:
        pass

    dataset_id = str(config.get("dataset_id") or query_config.get("dataset_id") or "")

    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "dataset_id": dataset_id,
        "dataset_name": row.get("dataset_name"),
        "chart_type": row.get("chart_type") or "table",
        "config": config,
        "query_config": query_config,
        "viz_config": viz_config,
        "sql_text": None,
        "visibility": row.get("visibility") or "internal",
        "created_at": row.get("created_at"),
        "updated_at": row.get("updated_at"),
        "created_by": row.get("created_by"),
        "owner": row.get("created_by"),
        "modified_by": row.get("updated_by") or row.get("created_by"),
        "favorite": row.get("favorite") == 1,
    }


def _vis_clause(role_idx: int, email_idx: int, alias: str = "c") -> str:
    return (
        f"({alias}.visibility = 'published' "
        f"OR ({alias}.visibility = 'internal' AND @param{role_idx} IN ('Analyst', 'Editor', 'Admin')) "
        f"OR ({alias}.visibility = 'private' AND {alias}.created_by = @param{email_idx}) "
        f"OR @param{role_idx} = 'Admin')"
    )


def list_charts(user_email: str, role: str = "Viewer") -> List[dict]:
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
               c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at,
               c.visibility, ds.dataset_name,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(c.id AS NVARCHAR(255))
            AND f.object_type = 'chart' AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds ON ds.id = TRY_CAST(JSON_VALUE(c.query_config, '$.dataset_id') AS INT)
        WHERE c.id IS NOT NULL AND {vis}
        ORDER BY c.updated_at DESC
    """, [user_email, role])
    return [_adapt(r) for r in result["rows"]]


def get_chart_by_id(chart_id: str, user_email: Optional[str] = None, role: str = "Admin") -> Optional[dict]:
    """
    role defaults to 'Admin' for internal service calls so chart rendering
    is never blocked by visibility. Pass the actual role from user-facing endpoints.
    """
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
                   c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at,
                   c.visibility
            FROM dbo.charts c
            WHERE c.id = @param0 AND c.id IS NOT NULL AND {vis}
        """, [int(chart_id), user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, description, chart_type, query_config, viz_config,
                   created_on, created_by, changed_on, updated_by, created_at, updated_at, visibility
            FROM dbo.charts WHERE id = @param0 AND id IS NOT NULL
        """, [int(chart_id)])
    return _adapt(row) if row else None


def create_chart(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    query_config = data.get("query_config") or {}
    if "dataset_id" in data and data["dataset_id"] is not None:
        query_config = {**query_config, "dataset_id": data["dataset_id"]}

    visibility = data.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        visibility = "internal"

    db.execute("""
        INSERT INTO charts (name, description, chart_type, query_config, viz_config,
                           visibility, created_on, created_by, changed_on, updated_by, created_at, updated_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8, @param9, @param10, @param11)
    """, [
        data["name"], data.get("description"), data["chart_type"],
        json.dumps(query_config),
        json.dumps(data.get("viz_config") or {}),
        visibility,
        now, user_id, now, user_id, now, now,
    ])
    inserted = db.query_one(
        "SELECT TOP 1 id FROM charts WHERE name = @param0 AND created_by = @param1 ORDER BY id DESC",
        [data["name"], user_id],
    )
    if not inserted:
        raise RuntimeError("Failed to retrieve created chart")
    return get_chart_by_id(str(inserted["id"]))


def update_chart(chart_id: str, data: dict) -> Optional[dict]:
    if not get_chart_by_id(chart_id):
        return None
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0

    for field_name, col in [("name", "name"), ("description", "description"), ("chart_type", "chart_type")]:
        if field_name in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field_name]); i += 1
    if "query_config" in data:
        updates.append(f"query_config = @param{i}"); params.append(json.dumps(data["query_config"])); i += 1
    if "viz_config" in data:
        updates.append(f"viz_config = @param{i}"); params.append(json.dumps(data["viz_config"])); i += 1
    if "visibility" in data:
        vis = data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"
        updates.append(f"visibility = @param{i}"); params.append(vis); i += 1

    updates.append(f"changed_on = @param{i}"); params.append(now); i += 1
    updates.append(f"updated_at = @param{i}"); params.append(now); i += 1
    params.append(int(chart_id))

    db.execute(f"UPDATE charts SET {', '.join(updates)} WHERE id = @param{i}", params)
    return get_chart_by_id(chart_id)


def delete_chart(chart_id: str) -> bool:
    return db.execute("DELETE FROM charts WHERE id = @param0", [int(chart_id)]) > 0


def count_charts() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM charts")
    return result.get("count") or 0
