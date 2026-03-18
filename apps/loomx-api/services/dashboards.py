"""Dashboards service — port of dashboards.service.ts."""

import json
import uuid
import re
from datetime import datetime
from typing import List, Optional
import database.metadata as db


def _adapt(row: dict) -> dict:
    layout = "[]"
    try:
        parsed = json.loads(row.get("layout") or "[]")
        layout = json.dumps(parsed)
    except Exception:
        pass

    return {
        "id": row.get("id"),
        "name": row.get("name"),
        "description": row.get("description"),
        "layout": layout,
        "charts": row.get("charts") or "[]",
        "filters": row.get("filters") or "[]",
        "created_at": row.get("created_at"),
        "updated_at": row.get("modified_at"),
        "created_by": row.get("created_by"),
        "owner": row.get("created_by"),
        "modified_by": row.get("modified_by") or row.get("created_by"),
        "is_published": bool(row.get("is_published")),
        "is_archived": bool(row.get("is_archived")),
        "favorite": row.get("favorite") == 1,
    }


def list_dashboards(user_id: Optional[str] = None) -> List[dict]:
    if user_id:
        result = db.query("""
            SELECT d.id, d.name, d.slug, d.description, d.layout, d.charts, d.filters,
                   d.theme, d.tags, d.is_published, d.is_archived,
                   d.created_by, d.modified_by, d.created_at, d.modified_at,
                   CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
            FROM dbo.dashboards d
            LEFT JOIN dbo.favorites f ON f.object_id = CAST(d.id AS NVARCHAR(255))
                AND f.object_type = 'dashboard' AND f.user_email = @param0
            WHERE d.id IS NOT NULL ORDER BY d.modified_at DESC
        """, [user_id])
    else:
        result = db.query("""
            SELECT id, name, slug, description, layout, charts, filters,
                   theme, tags, is_published, is_archived,
                   created_by, modified_by, created_at, modified_at, 0 as favorite
            FROM dbo.dashboards WHERE id IS NOT NULL ORDER BY modified_at DESC
        """)
    return [_adapt(r) for r in result["rows"]]


def get_dashboard_by_id(dashboard_id: str) -> Optional[dict]:
    row = db.query_one("""
        SELECT id, name, slug, description, layout, charts, filters,
               theme, tags, is_published, is_archived,
               created_by, modified_by, created_at, modified_at
        FROM dbo.dashboards WHERE id = @param0 AND id IS NOT NULL
    """, [dashboard_id])
    return _adapt(row) if row else None


def create_dashboard(data: dict, user_id: str) -> dict:
    d_id = str(uuid.uuid4())
    now = datetime.utcnow()
    slug = re.sub(r"[^a-z0-9-]", "", data["name"].lower().replace(" ", "-"))

    def _to_str(v):
        if isinstance(v, str):
            return v
        return json.dumps(v) if v is not None else "[]"

    db.execute("""
        INSERT INTO dashboards (id, name, slug, description, layout, charts, filters,
                               theme, tags, is_published, is_archived,
                               created_by, modified_by, created_at, modified_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6,
                @param7, @param8, @param9, @param10, @param11, @param12, @param13, @param14)
    """, [
        d_id, data["name"], slug, data.get("description"),
        _to_str(data.get("layout", [])),
        _to_str(data.get("charts", [])),
        _to_str(data.get("filters", [])),
        None, None,
        1 if data.get("is_published") else 0,
        1 if data.get("is_archived") else 0,
        user_id, user_id, now, now,
    ])
    created = get_dashboard_by_id(d_id)
    if not created:
        raise RuntimeError("Failed to retrieve created dashboard")
    return created


def update_dashboard(dashboard_id: str, data: dict) -> Optional[dict]:
    if not get_dashboard_by_id(dashboard_id):
        return None
    now = datetime.utcnow()
    updates, params, i = [], [], 0

    if "name" in data:
        updates.append(f"name = @param{i}"); params.append(data["name"]); i += 1
        slug = re.sub(r"[^a-z0-9-]", "", data["name"].lower().replace(" ", "-"))
        updates.append(f"slug = @param{i}"); params.append(slug); i += 1
    if "description" in data:
        updates.append(f"description = @param{i}"); params.append(data["description"]); i += 1

    def _to_str(v):
        return v if isinstance(v, str) else json.dumps(v)

    for field, col in [("layout", "layout"), ("charts", "charts"), ("filters", "filters")]:
        if field in data:
            updates.append(f"{col} = @param{i}"); params.append(_to_str(data[field])); i += 1
    if "is_published" in data:
        updates.append(f"is_published = @param{i}"); params.append(1 if data["is_published"] else 0); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    params.append(dashboard_id)

    db.execute(f"UPDATE dashboards SET {', '.join(updates)} WHERE id = @param{i}", params)
    return get_dashboard_by_id(dashboard_id)


def delete_dashboard(dashboard_id: str) -> bool:
    return db.execute("DELETE FROM dashboards WHERE id = @param0", [dashboard_id]) > 0


def count_dashboards() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM dashboards")
    return result.get("count") or 0
