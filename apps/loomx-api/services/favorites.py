"""Favorites service — port of favorites.service.ts."""

import uuid
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db


def list_favorites(user_id: str) -> List[dict]:
    charts = db.query("""
        SELECT
            f.id as favorite_id, 'chart' as kind,
            c.id, c.name, c.created_by as owner,
            c.created_at, c.updated_at, f.created_at as favorited_at
        FROM favorites f
        INNER JOIN dbo.charts c ON CAST(c.id AS NVARCHAR(255)) = f.object_id
        WHERE f.user_email = @param0 AND f.object_type = 'chart'
    """, [user_id])["rows"]

    datasets = db.query("""
        SELECT
            f.id as favorite_id, 'dataset' as kind,
            d.id, d.dataset_name as name, d.created_by as owner,
            d.created_at, d.modified_at as updated_at, f.created_at as favorited_at
        FROM favorites f
        INNER JOIN dbo.datasets d ON CAST(d.id AS NVARCHAR(255)) = f.object_id
        WHERE f.user_email = @param0 AND f.object_type = 'dataset'
    """, [user_id])["rows"]

    dashboards = db.query("""
        SELECT
            f.id as favorite_id, 'dashboard' as kind,
            d.id, d.name, d.created_by as owner,
            d.created_at, d.modified_at as updated_at, f.created_at as favorited_at
        FROM favorites f
        INNER JOIN dbo.dashboards d ON d.id = f.object_id
        WHERE f.user_email = @param0 AND f.object_type = 'dashboard'
    """, [user_id])["rows"]

    all_favs = charts + datasets + dashboards
    all_favs.sort(key=lambda x: x.get("favorited_at") or datetime.min, reverse=True)
    return all_favs


def is_favorite(user_id: str, object_type: str, object_id: str) -> bool:
    result = db.query_one("""
        SELECT COUNT(*) as count FROM favorites
        WHERE user_email = @param0 AND object_type = @param1 AND object_id = @param2
    """, [user_id, object_type, object_id])
    return (result.get("count") or 0) > 0


def create_favorite(data: dict, user_id: str) -> dict:
    if is_favorite(user_id, data["object_type"], data["object_id"]):
        existing = db.query_one("""
            SELECT id, user_email, object_id, object_type, object_name, created_at
            FROM favorites
            WHERE user_email = @param0 AND object_type = @param1 AND object_id = @param2
        """, [user_id, data["object_type"], data["object_id"]])
        if existing:
            return _adapt(existing)

    fav_id = str(uuid.uuid4())
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    db.execute("""
        INSERT INTO favorites (id, user_email, object_id, object_type, object_name, created_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5)
    """, [fav_id, user_id, data["object_id"], data["object_type"], data["object_name"], now])

    return {
        "id": fav_id,
        "object_type": data["object_type"],
        "object_id": data["object_id"],
        "object_name": data["object_name"],
        "created_at": now,
        "user_id": user_id,
    }


def toggle_favorite(data: dict, user_id: str) -> dict:
    if is_favorite(user_id, data["object_type"], data["object_id"]):
        delete_favorite(user_id, data["object_type"], data["object_id"])
        return {"favorited": False}
    fav = create_favorite(data, user_id)
    return {"favorited": True, "favorite": fav}


def delete_favorite(user_id: str, object_type: str, object_id: str) -> bool:
    count = db.execute("""
        DELETE FROM favorites
        WHERE user_email = @param0 AND object_type = @param1 AND object_id = @param2
    """, [user_id, object_type, object_id])
    return count > 0


def delete_favorite_by_id(fav_id: str, user_id: str) -> bool:
    count = db.execute("""
        DELETE FROM favorites WHERE id = @param0 AND user_email = @param1
    """, [fav_id, user_id])
    return count > 0


def _adapt(row: dict) -> dict:
    return {
        "id": row.get("id"),
        "object_type": row.get("object_type"),
        "object_id": row.get("object_id"),
        "object_name": row.get("object_name"),
        "created_at": row.get("created_at"),
        "user_id": row.get("user_email"),
    }
