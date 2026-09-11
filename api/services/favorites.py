"""Favorites service — port of favorites.service.ts."""

import uuid
import hashlib
import logging
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db
from services import product_outbox, product_shadow_read

MIGRATABLE_TYPES = {"dataset", "chart", "dashboard", "saved_query"}

def _record_id(owner, object_type, object_id):
    return hashlib.sha256(f"{owner}\0{object_type}\0{object_id}".encode()).hexdigest()

def _document(owner, row):
    return {"user_email": owner, "object_type": str(row["object_type"]),
            "object_id": str(row["object_id"]), "object_name": row.get("object_name")}


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
    try:
        report=product_shadow_read.observe_favorite_list(all_favs,user_id)
        if report.get("enabled"): logging.getLogger(__name__).info("favorite_shadow_read %s",report)
    except Exception as error:
        logging.getLogger(__name__).warning("favorite_shadow_read_error type=%s",type(error).__name__)
    return all_favs


def is_favorite(user_id: str, object_type: str, object_id: str) -> bool:
    result = db.query_one("""
        SELECT COUNT(*) as count FROM favorites
        WHERE user_email = @param0 AND object_type = @param1 AND object_id = @param2
    """, [user_id, object_type, object_id])
    return (result.get("count") or 0) > 0


def create_favorite(data: dict, user_id: str) -> dict:
    if data["object_type"] not in MIGRATABLE_TYPES and data["object_type"] != "data_source":
        raise ValueError("Unsupported favorite object type")
    with db.transaction() as transaction:
        existing = transaction.query_one("""
            SELECT id, user_email, object_id, object_type, object_name, created_at
            FROM favorites
            WHERE user_email = @param0 AND object_type = @param1 AND object_id = @param2
            FOR UPDATE
        """, [user_id, data["object_type"], data["object_id"]])
        if existing:
            return _adapt(existing)
        fav_id = str(uuid.uuid4())
        now = datetime.now(timezone.utc).replace(tzinfo=None)
        transaction.execute("""INSERT INTO favorites (id, user_email, object_id, object_type, object_name, created_at)
            VALUES (@param0, @param1, @param2, @param3, @param4, @param5)""",
            [fav_id,user_id,data["object_id"],data["object_type"],data["object_name"],now])
        if data["object_type"] in MIGRATABLE_TYPES:
            document=_document(user_id,data)
            product_outbox.enqueue(transaction,family="favorites",operation="create",
                record_id=_record_id(user_id,data["object_type"],str(data["object_id"])),payload=document,
                actor=user_id,owner=user_id)

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
    with db.transaction() as transaction:
        row=transaction.query_one("""SELECT id,object_type,object_id FROM favorites
            WHERE user_email=@param0 AND object_type=@param1 AND object_id=@param2 FOR UPDATE""",
            [user_id,object_type,object_id])
        if not row: return False
        transaction.execute("DELETE FROM favorites WHERE id=@param0",[row["id"]])
        if object_type in MIGRATABLE_TYPES:
            product_outbox.enqueue(transaction,family="favorites",operation="delete",
                record_id=_record_id(user_id,object_type,str(object_id)),payload={},actor=user_id,owner=user_id)
        return True


def delete_favorite_by_id(fav_id: str, user_id: str) -> bool:
    with db.transaction() as transaction:
        row=transaction.query_one("SELECT id,object_type,object_id FROM favorites WHERE id=@param0 AND user_email=@param1 FOR UPDATE",[fav_id,user_id])
        if not row: return False
        transaction.execute("DELETE FROM favorites WHERE id=@param0",[fav_id])
        if row["object_type"] in MIGRATABLE_TYPES:
            product_outbox.enqueue(transaction,family="favorites",operation="delete",
                record_id=_record_id(user_id,row["object_type"],str(row["object_id"])),payload={},actor=user_id,owner=user_id)
        return True


def _adapt(row: dict) -> dict:
    return {
        "id": row.get("id"),
        "object_type": row.get("object_type"),
        "object_id": row.get("object_id"),
        "object_name": row.get("object_name"),
        "created_at": row.get("created_at"),
        "user_id": row.get("user_email"),
    }
