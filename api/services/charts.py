"""Charts service — port of charts.service.ts."""

import json
import threading
import time
import uuid
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db

VALID_VISIBILITY = {"private", "internal", "published"}
_SCHEMA_CACHE_TTL_SECONDS = 60.0
_schema_cache: tuple[float, str] | None = None
_schema_lock = threading.Lock()


def _chart_schema() -> str:
    """Return one known charts layout from a bounded, read-only capability probe."""
    global _schema_cache
    now = time.monotonic()
    with _schema_lock:
        if _schema_cache and now - _schema_cache[0] < _SCHEMA_CACHE_TTL_SECONDS:
            return _schema_cache[1]
        rows = db.query(
            "SELECT column_name FROM information_schema.columns WHERE table_name = @param0",
            ["charts"],
        )["rows"]
        columns = {str(row.get("column_name", "")).casefold() for row in rows}
        modern = {"id", "dataset_id", "config", "modified_at", "modified_by"}
        legacy = {"id", "query_config", "viz_config", "updated_at", "updated_by"}
        if modern <= columns:
            layout = "modern"
        elif legacy <= columns:
            layout = "legacy"
        else:
            raise RuntimeError("Charts metadata schema is not a supported layout")
        _schema_cache = (now, layout)
        return layout


def _configs(row: dict) -> tuple[dict, dict]:
    """Read the PostgreSQL `config` envelope and tolerate legacy flat config."""
    stored = {}
    try:
        stored = json.loads(row.get("config") or "{}")
    except Exception:
        pass
    if not isinstance(stored, dict):
        stored = {}
    query_config = stored.get("query_config")
    viz_config = stored.get("viz_config")
    # Earlier installations stored the query config directly in `config`.
    if not isinstance(query_config, dict):
        query_config = stored
    if not isinstance(viz_config, dict):
        viz_config = {}
    return query_config, viz_config


def _adapt(row: dict, layout: str) -> dict:
    if layout == "legacy":
        try:
            query_config = json.loads(row.get("query_config") or "{}")
            viz_config = json.loads(row.get("viz_config") or "{}")
        except Exception:
            query_config, viz_config = {}, {}
    else:
        query_config, viz_config = _configs(row)
    config = {**query_config, **viz_config}

    dataset_id = str(row.get("dataset_id") or query_config.get("dataset_id") or "")

    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "dataset_id": dataset_id,
        "dataset_name": row.get("dataset_name"),
        "chart_type": row.get("chart_type") or "table",
        "thumbnail": row.get("thumbnail"),
        "config": config,
        "query_config": query_config,
        "viz_config": viz_config,
        "sql_text": None,
        "visibility": row.get("visibility") or "internal",
        "created_at": row.get("created_at"),
        "updated_at": row.get("updated_at"),
        "created_by": row.get("created_by"),
        "owner": row.get("created_by"),
        "modified_by": (row.get("updated_by") if layout == "legacy" else row.get("modified_by")) or row.get("created_by"),
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
    layout = _chart_schema()
    if layout == "legacy":
        return _legacy_list_charts(user_email, role)
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT c.id, c.name, c.description, c.dataset_id, c.chart_type, c.config,
               c.created_by, c.modified_by, c.created_at, c.modified_at,
               c.visibility, c.thumbnail, ds.dataset_name,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(c.id AS NVARCHAR(255))
            AND f.object_type = 'chart' AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds ON ds.id = c.dataset_id
        WHERE c.id IS NOT NULL AND {vis}
        ORDER BY c.modified_at DESC
    """, [user_email, role])
    return [_adapt(r, layout) for r in result["rows"]]


def get_chart_by_id(chart_id: str, user_email: Optional[str] = None, role: str = "Admin") -> Optional[dict]:
    """
    role defaults to 'Admin' for internal service calls so chart rendering
    is never blocked by visibility. Pass the actual role from user-facing endpoints.
    """
    layout = _chart_schema()
    if layout == "legacy":
        return _legacy_get_chart(chart_id, user_email, role)
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT c.id, c.name, c.description, c.dataset_id, c.chart_type, c.config,
                   c.created_by, c.modified_by, c.created_at, c.modified_at,
                   c.visibility
            FROM dbo.charts c
            WHERE c.id = @param0 AND c.id IS NOT NULL AND {vis}
        """, [chart_id, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, description, dataset_id, chart_type, config,
                   created_by, modified_by, created_at, modified_at, visibility
            FROM dbo.charts WHERE id = @param0 AND id IS NOT NULL
        """, [chart_id])
    return _adapt(row, layout) if row else None


def create_chart(data: dict, user_id: str) -> dict:
    if _chart_schema() == "legacy":
        return _legacy_create_chart(data, user_id)
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    query_config = data.get("query_config") or data.get("config") or {}
    if "dataset_id" in data and data["dataset_id"] is not None:
        query_config = {**query_config, "dataset_id": data["dataset_id"]}

    visibility = data.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        visibility = "internal"

    chart_id = str(uuid.uuid4())
    envelope = {"query_config": query_config, "viz_config": data.get("viz_config") or {}}
    db.execute("""
        INSERT INTO charts (id, name, description, dataset_id, chart_type, config,
                           visibility, created_by, modified_by, created_at, modified_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8, @param9, @param10)
    """, [
        chart_id, data["name"], data.get("description"), data["dataset_id"], data["chart_type"],
        json.dumps(envelope), visibility, user_id, user_id, now, now,
    ])
    created = get_chart_by_id(chart_id)
    if not created:
        raise RuntimeError("Failed to retrieve created chart")
    return created


def update_chart(chart_id: str, data: dict) -> Optional[dict]:
    if _chart_schema() == "legacy":
        return _legacy_update_chart(chart_id, data)
    if not get_chart_by_id(chart_id):
        return None
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0

    for field_name, col in [("name", "name"), ("description", "description"), ("chart_type", "chart_type")]:
        if field_name in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field_name]); i += 1
    if "dataset_id" in data:
        updates.append(f"dataset_id = @param{i}"); params.append(data["dataset_id"]); i += 1
    if "query_config" in data or "viz_config" in data or "config" in data:
        existing = get_chart_by_id(chart_id) or {}
        query_config = data.get("query_config", data.get("config", existing.get("query_config") or {}))
        viz_config = data.get("viz_config", existing.get("viz_config") or {})
        updates.append(f"config = @param{i}")
        params.append(json.dumps({"query_config": query_config or {}, "viz_config": viz_config or {}})); i += 1
    if "visibility" in data:
        vis = data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"
        updates.append(f"visibility = @param{i}"); params.append(vis); i += 1
    if "thumbnail" in data:
        updates.append(f"thumbnail = @param{i}"); params.append(data["thumbnail"]); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    params.append(chart_id)

    db.execute(f"UPDATE charts SET {', '.join(updates)} WHERE id = @param{i}", params)
    return get_chart_by_id(chart_id)


def delete_chart(chart_id: str) -> bool:
    if _chart_schema() == "legacy":
        return db.execute("DELETE FROM charts WHERE id = @param0", [int(chart_id)]) > 0
    return db.execute("DELETE FROM charts WHERE id = @param0", [chart_id]) > 0


def count_charts() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM charts")
    return result.get("count") or 0


def _legacy_list_charts(user_email: str, role: str) -> List[dict]:
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
               c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at,
               c.visibility, c.thumbnail, ds.dataset_name,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(c.id AS NVARCHAR(255))
            AND f.object_type = 'chart' AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds ON ds.id = TRY_CAST(JSON_VALUE(c.query_config, '$.dataset_id') AS INT)
        WHERE c.id IS NOT NULL AND {vis}
        ORDER BY c.updated_at DESC
    """, [user_email, role])
    return [_adapt(row, "legacy") for row in result["rows"]]


def _legacy_get_chart(chart_id: str, user_email: Optional[str], role: str) -> Optional[dict]:
    chart_id_int = int(chart_id)
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
                   c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at, c.visibility
            FROM dbo.charts c WHERE c.id = @param0 AND c.id IS NOT NULL AND {vis}
        """, [chart_id_int, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, description, chart_type, query_config, viz_config,
                   created_on, created_by, changed_on, updated_by, created_at, updated_at, visibility
            FROM dbo.charts WHERE id = @param0 AND id IS NOT NULL
        """, [chart_id_int])
    return _adapt(row, "legacy") if row else None


def _legacy_create_chart(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    query_config = data.get("query_config") or data.get("config") or {}
    if data.get("dataset_id") is not None:
        query_config = {**query_config, "dataset_id": data["dataset_id"]}
    visibility = data.get("visibility") if data.get("visibility") in VALID_VISIBILITY else "internal"
    db.execute("""
        INSERT INTO charts (name, description, chart_type, query_config, viz_config, visibility,
                            created_on, created_by, changed_on, updated_by, created_at, updated_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8, @param9, @param10, @param11)
    """, [data["name"], data.get("description"), data["chart_type"], json.dumps(query_config),
            json.dumps(data.get("viz_config") or {}), visibility, now, user_id, now, user_id, now, now])
    inserted = db.query_one("SELECT TOP 1 id FROM charts WHERE name = @param0 AND created_by = @param1 ORDER BY id DESC", [data["name"], user_id])
    if not inserted:
        raise RuntimeError("Failed to retrieve created chart")
    return _legacy_get_chart(str(inserted["id"]), None, "Admin")


def _legacy_update_chart(chart_id: str, data: dict) -> Optional[dict]:
    if not _legacy_get_chart(chart_id, None, "Admin"):
        return None
    now, updates, params, index = datetime.now(timezone.utc).replace(tzinfo=None), [], [], 0
    for field, column in [("name", "name"), ("description", "description"), ("chart_type", "chart_type")]:
        if field in data:
            updates.append(f"{column} = @param{index}"); params.append(data[field]); index += 1
    for field in ("query_config", "viz_config"):
        if field in data:
            updates.append(f"{field} = @param{index}"); params.append(json.dumps(data[field])); index += 1
    if "visibility" in data:
        updates.append(f"visibility = @param{index}"); params.append(data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"); index += 1
    if "thumbnail" in data:
        updates.append(f"thumbnail = @param{index}"); params.append(data["thumbnail"]); index += 1
    updates.append(f"changed_on = @param{index}"); params.append(now); index += 1
    updates.append(f"updated_at = @param{index}"); params.append(now); index += 1
    params.append(int(chart_id))
    db.execute(f"UPDATE charts SET {', '.join(updates)} WHERE id = @param{index}", params)
    return _legacy_get_chart(chart_id, None, "Admin")
