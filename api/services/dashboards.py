"""Dashboards service — port of dashboards.service.ts."""

import json
import os
import uuid
import re
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db
from services import dashboard_backfill, product_outbox, product_shadow_read, product_store

VALID_VISIBILITY = {"private", "internal", "published"}

_thumb_dark_ready = False


def _outbox_enabled() -> bool:
    return os.getenv("KAVEON_DASHBOARD_OUTBOX_ENABLED") == "true"


def _outbox_document(transaction, dashboard_id: str) -> dict:
    row = transaction.query_one("""SELECT id,name,description,layout,charts,filters,theme,visibility,
        is_published,is_archived,created_by,modified_by,created_at,modified_at
        FROM dashboards WHERE id=@param0""", [dashboard_id])
    if not row:
        raise RuntimeError("Dashboard disappeared before outbox capture")
    owner, record_id = str(row.get("created_by") or ""), str(row["id"])
    if not owner:
        raise RuntimeError("Dashboard owner is unavailable for outbox capture")
    chart_ids = dashboard_backfill._json(row.get("charts"), "charts", [])
    if len(chart_ids) > dashboard_backfill.MAX_CHART_REFS or len({str(value) for value in chart_ids}) != len(chart_ids):
        raise RuntimeError("Dashboard chart references are invalid")
    revisions = {}
    for chart_id in sorted(str(value) for value in chart_ids):
        chart = product_store.read("chart", chart_id, owner, "Admin")
        revision = chart.get("revision") if chart else None
        if type(revision) is not int or revision < 1:
            raise RuntimeError(f"KaveonDB chart {chart_id} revision is unavailable for dashboard outbox capture")
        revisions[chart_id] = revision
    visibility = row.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        raise RuntimeError("Dashboard visibility is invalid")
    return {"id": record_id, "name": row.get("name"), "description": row.get("description"),
        "layout": dashboard_backfill._json(row.get("layout"), "layout", []), "charts": chart_ids,
        "chart_revisions": revisions, "filters": dashboard_backfill._json(row.get("filters"), "filters", []),
        "theme": row.get("theme"), "visibility": visibility,
        "is_published": bool(row.get("is_published")), "is_archived": bool(row.get("is_archived")),
        "created_by": owner, "modified_by": str(row.get("modified_by") or owner),
        "created_at": dashboard_backfill._text(row.get("created_at")),
        "updated_at": dashboard_backfill._text(row.get("modified_at"))}


def _enqueue(transaction, operation: str, dashboard_id: str, actor: str, owner: str | None = None) -> None:
    document = {} if operation == "delete" else _outbox_document(transaction, dashboard_id)
    product_outbox.enqueue(transaction, family="dashboards", operation=operation, record_id=dashboard_id,
                           payload=document, actor=actor, owner=owner or document.get("created_by") or actor)


def _ensure_thumbnail_dark_column() -> None:
    """Self-migrate: add dashboards.thumbnail_dark for the second (dark-mode)
    thumbnail. Idempotent (ADD COLUMN IF NOT EXISTS); best-effort so a metadata
    store that lacks the syntax simply degrades to a single (light) thumbnail."""
    global _thumb_dark_ready
    if _thumb_dark_ready:
        return
    try:
        db.execute("ALTER TABLE dbo.dashboards ADD COLUMN IF NOT EXISTS thumbnail_dark TEXT")
    except Exception:
        pass
    _thumb_dark_ready = True


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
        "theme": row.get("theme"),
        "thumbnail": row.get("thumbnail"),
        "thumbnail_dark": row.get("thumbnail_dark"),
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


def _adapt_product(document: dict) -> dict:
    result = dict(document)
    for field in ("layout", "charts", "filters"):
        value = result.get(field, [])
        if not isinstance(value, (list, dict)):
            raise RuntimeError(f"KaveonDB dashboard {field} is invalid")
        result[field] = json.dumps(value)
    result.update({
        "thumbnail": None, "thumbnail_dark": None,
        "owner": result.get("created_by"), "favorite": bool(result.get("favorite", False)),
    })
    return result


def _vis_clause(role_idx: int, email_idx: int, alias: str = "d") -> str:
    return (
        f"({alias}.visibility = 'published' "
        f"OR ({alias}.visibility = 'internal' AND @param{role_idx} IN ('Analyst', 'Editor', 'Admin')) "
        f"OR ({alias}.visibility = 'private' AND {alias}.created_by = @param{email_idx}) "
        f"OR @param{role_idx} = 'Admin')"
    )


def list_dashboards(user_email: str, role: str = "Viewer") -> List[dict]:
    from services import product_read_authority
    if product_read_authority.enabled("dashboards"):
        return [_adapt_product(item) for item in
                product_read_authority.list_documents("dashboards", user_email, role)]
    _ensure_thumbnail_dark_column()
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT d.id, d.name, d.slug, d.description, d.layout, d.charts, d.filters,
               d.theme, d.tags, d.thumbnail, d.thumbnail_dark, d.is_published, d.is_archived, d.visibility,
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
    from services import product_read_authority
    if product_read_authority.enabled("dashboards"):
        document = product_read_authority.read_document(
            "dashboards", dashboard_id, user_email, role,
        )
        if document is not None:
            document.setdefault("favorite", False)
            return _adapt_product(document)
        return None
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
    dashboard = _adapt(row) if row else None
    if dashboard is not None and user_email:
        try:
            report = product_shadow_read.compare_dashboard(dashboard, user_email, role)
            if report.get("enabled"):
                import logging
                logging.getLogger(__name__).info("dashboard_shadow_read %s", json.dumps(report, sort_keys=True))
        except Exception as error:
            import logging
            logging.getLogger(__name__).warning("dashboard_shadow_read_error type=%s", type(error).__name__)
    return dashboard


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

    statement = """
        INSERT INTO dashboards (id, name, slug, description, layout, charts, filters,
                               theme, tags, visibility, is_published, is_archived,
                               created_by, modified_by, created_at, modified_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6,
                @param7, @param8, @param9, @param10, @param11, @param12, @param13, @param14, @param15)
    """
    params = [
        d_id, data["name"], slug, data.get("description"),
        _to_str(data.get("layout", [])),
        _to_str(data.get("charts", [])),
        _to_str(data.get("filters", [])),
        data.get("theme"), None, visibility,
        bool(data.get("is_published")),
        bool(data.get("is_archived")),
        user_id, user_id, now, now,
    ]
    if _outbox_enabled():
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            _enqueue(transaction, "create", d_id, user_id)
    else:
        db.execute(statement, params)
    created = get_dashboard_by_id(d_id)
    if not created:
        raise RuntimeError("Failed to retrieve created dashboard")
    return created


def update_dashboard(dashboard_id: str, data: dict, actor: str | None = None) -> Optional[dict]:
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
    if "theme" in data:
        updates.append(f"theme = @param{i}"); params.append(data["theme"]); i += 1
    if "thumbnail" in data:
        updates.append(f"thumbnail = @param{i}"); params.append(data["thumbnail"]); i += 1
    if "thumbnail_dark" in data:
        _ensure_thumbnail_dark_column()
        updates.append(f"thumbnail_dark = @param{i}"); params.append(data["thumbnail_dark"]); i += 1

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

    statement = f"UPDATE dashboards SET {', '.join(updates)} WHERE id = @param{i}"
    if _outbox_enabled():
        if not actor:
            raise RuntimeError("Dashboard outbox capture requires actor identity")
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            _enqueue(transaction, "update", dashboard_id, actor)
    else:
        db.execute(statement, params)
    return get_dashboard_by_id(dashboard_id)


def delete_dashboard(dashboard_id: str, actor: str | None = None) -> bool:
    if not _outbox_enabled():
        return db.execute("DELETE FROM dashboards WHERE id = @param0", [dashboard_id]) > 0
    if not actor:
        raise RuntimeError("Dashboard outbox capture requires actor identity")
    with db.transaction() as transaction:
        row = transaction.query_one("SELECT created_by FROM dashboards WHERE id=@param0 FOR UPDATE", [dashboard_id])
        if not row:
            return False
        deleted = transaction.execute("DELETE FROM dashboards WHERE id = @param0", [dashboard_id]) > 0
        _enqueue(transaction, "delete", dashboard_id, actor, str(row["created_by"]))
        return deleted


def count_dashboards() -> int:
    result = db.query_one("SELECT COUNT(*) as count FROM dashboards")
    return result.get("count") or 0
