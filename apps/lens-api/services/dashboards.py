"""Dashboards service — port of dashboards.service.ts."""

import json
import uuid
import re
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db

VALID_VISIBILITY = {"private", "internal", "published"}


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
        "visibility": row.get("visibility") or "internal",
        "created_at": row.get("created_at"),
        "updated_at": row.get("modified_at"),
        "created_by": row.get("created_by"),
        "owner": row.get("created_by"),
        "modified_by": row.get("modified_by") or row.get("created_by"),
        "is_published": bool(row.get("is_published")),
        "is_archived": bool(row.get("is_archived")),
        "favorite": row.get("favorite") == 1,
    }


def _vis_clause(role_idx: int, email_idx: int, alias: str = "d") -> str:
    return (
        f"({alias}.visibility = 'published' "
        f"OR ({alias}.visibility = 'internal' AND @param{role_idx} IN ('Analyst', 'Editor', 'Admin')) "
        f"OR ({alias}.visibility = 'private' AND {alias}.created_by = @param{email_idx}) "
        f"OR @param{role_idx} = 'Admin')"
    )


def list_dashboards(user_email: str, role: str = "Viewer") -> List[dict]:
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT d.id, d.name, d.slug, d.description, d.layout, d.charts, d.filters,
               d.theme, d.tags, d.is_published, d.is_archived, d.visibility,
               d.created_by, d.modified_by, d.created_at, d.modified_at,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.dashboards d
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(d.id AS NVARCHAR(255))
            AND f.object_type = 'dashboard' AND f.user_email = @param0
        WHERE d.id IS NOT NULL AND {vis}
        ORDER BY d.modified_at DESC
    """, [user_email, role])
    return [_adapt(r) for r in result["rows"]]


def get_dashboard_by_id(
    dashboard_id: str,
    user_email: Optional[str] = None,
    role: str = "Admin",
) -> Optional[dict]:
    """role defaults to 'Admin' for internal/rendering calls."""
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT d.id, d.name, d.slug, d.description, d.layout, d.charts, d.filters,
                   d.theme, d.tags, d.is_published, d.is_archived, d.visibility,
                   d.created_by, d.modified_by, d.created_at, d.modified_at
            FROM dbo.dashboards d
            WHERE d.id = @param0 AND d.id IS NOT NULL AND {vis}
        """, [dashboard_id, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, slug, description, layout, charts, filters,
                   theme, tags, is_published, is_archived, visibility,
                   created_by, modified_by, created_at, modified_at
            FROM dbo.dashboards WHERE id = @param0 AND id IS NOT NULL
        """, [dashboard_id])
    return _adapt(row) if row else None


def create_dashboard(data: dict, user_id: str) -> dict:
    d_id = str(uuid.uuid4())
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    slug = re.sub(r"[^a-z0-9-]", "", data["name"].lower().replace(" ", "-"))

    visibility = data.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        visibility = "internal"

    def _to_str(v):
        if isinstance(v, str):
            return v
        return json.dumps(v) if v is not None else "[]"

    db.execute("""
        INSERT INTO dashboards (id, name, slug, description, layout, charts, filters,
                               theme, tags, visibility, is_published, is_archived,
                               created_by, modified_by, created_at, modified_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6,
                @param7, @param8, @param9, @param10, @param11, @param12, @param13, @param14, @param15)
    """, [
        d_id, data["name"], slug, data.get("description"),
        _to_str(data.get("layout", [])),
        _to_str(data.get("charts", [])),
        _to_str(data.get("filters", [])),
        None, None, visibility,
        bool(data.get("is_published")),
        bool(data.get("is_archived")),
        user_id, user_id, now, now,
    ])
    created = get_dashboard_by_id(d_id)
    if not created:
        raise RuntimeError("Failed to retrieve created dashboard")
    return created


def update_dashboard(dashboard_id: str, data: dict) -> Optional[dict]:
    if not get_dashboard_by_id(dashboard_id):
        return None
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0

    if "name" in data:
        updates.append(f"name = @param{i}"); params.append(data["name"]); i += 1
        slug = re.sub(r"[^a-z0-9-]", "", data["name"].lower().replace(" ", "-"))
        updates.append(f"slug = @param{i}"); params.append(slug); i += 1
    if "description" in data:
        updates.append(f"description = @param{i}"); params.append(data["description"]); i += 1

    def _to_str(v):
        return v if isinstance(v, str) else json.dumps(v)

    for field_name, col in [("layout", "layout"), ("charts", "charts"), ("filters", "filters")]:
        if field_name in data:
            updates.append(f"{col} = @param{i}"); params.append(_to_str(data[field_name])); i += 1
    if "is_published" in data:
        # Boolean, not 1/0 — Postgres won't implicitly cast int→boolean in a
        # parameterised UPDATE (MSSQL's bit column accepts bool fine too).
        updates.append(f"is_published = @param{i}"); params.append(bool(data["is_published"])); i += 1
    if "visibility" in data:
        vis = data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"
        updates.append(f"visibility = @param{i}"); params.append(vis); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    params.append(dashboard_id)

    db.execute(f"UPDATE dashboards SET {', '.join(updates)} WHERE id = @param{i}", params)
    return get_dashboard_by_id(dashboard_id)


def delete_dashboard(dashboard_id: str) -> bool:
    return db.execute("DELETE FROM dashboards WHERE id = @param0", [dashboard_id]) > 0


def count_dashboards() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM dashboards")
    return result.get("count") or 0
