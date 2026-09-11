"""Saved queries service — port of savedQueries.service.ts."""

from datetime import datetime, timezone
from typing import List, Optional
import logging
import os
import database.metadata as db
from services import product_outbox
from services import product_shadow_read

def _enqueue(transaction, **kwargs):
    if os.getenv("KAVEON_SAVED_QUERY_OUTBOX_ENABLED") == "true":
        return product_outbox.enqueue(transaction, **kwargs)


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
        # Pinned = a row in the shared favorites store (object_type 'query'),
        # same mechanism the Library pin uses for charts/dashboards/datasets.
        "favorite": bool(row.get("fav")),
    }


def _product_document(row: dict) -> dict:
    """Canonical KaveonDB payload shared by source mutations and backfill."""
    owner = str(row.get("created_by") or "")
    updated_at = row.get("modified_at", row.get("updated_at"))
    modified_by = str(row.get("modified_by") or owner)
    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "sql": row.get("sql_text"),
        "created_at": row.get("created_at").isoformat() if hasattr(row.get("created_at"), "isoformat") else row.get("created_at"),
        "updated_at": updated_at.isoformat() if hasattr(updated_at, "isoformat") else updated_at,
        "created_by": owner,
        "modified_by": modified_by,
    }


def list_saved_queries(user_id: str) -> List[dict]:
    result = db.query("""
        SELECT s.id, s.name, s.description, s.sql_text, s.created_by,
               s.created_at, s.modified_at,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END AS fav
        FROM saved_queries s
        LEFT JOIN favorites f ON f.object_id = CAST(s.id AS NVARCHAR(255))
            AND f.object_type = 'query' AND f.user_email = @param0
        WHERE s.created_by = @param0 ORDER BY s.modified_at DESC
    """, [user_id])
    adapted=[_adapt(r) for r in result["rows"]]
    try:
        report=product_shadow_read.observe_saved_query_list([_product_document(r) for r in result["rows"]],user_id)
        if report.get("enabled"):logging.getLogger(__name__).info("saved_query_shadow %s",report)
    except Exception as error:logging.getLogger(__name__).warning("saved_query_shadow_error type=%s",type(error).__name__)
    return adapted


def get_by_id(query_id: str, user_id: str) -> Optional[dict]:
    row = db.query_one(
        _SELECT + "WHERE id = @param0 AND created_by = @param1",
        [query_id, user_id],
    )
    if row:
        try:
            report=product_shadow_read.observe_saved_query(_product_document(row),user_id)
            if report.get("enabled"):logging.getLogger(__name__).info("saved_query_shadow %s",report)
        except Exception as error:logging.getLogger(__name__).warning("saved_query_shadow_error type=%s",type(error).__name__)
    return _adapt(row) if row else None


def create_saved_query(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    with db.transaction() as transaction:
        inserted = transaction.query_one("""
            INSERT INTO saved_queries (name, description, sql_text, created_by, created_at,
                                       modified_by, modified_at, is_shared, favorite)
            VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8)
            RETURNING id, name, description, sql_text, created_by, created_at,
                      modified_by, modified_at
        """, [data["name"], data.get("description"), data["sql"], user_id, now,
                user_id, now, False, False])
        if not inserted:
            raise RuntimeError("Failed to retrieve created saved query")
        _enqueue(
            transaction, family="saved_queries", operation="create",
            record_id=str(inserted["id"]), payload=_product_document(inserted),
            actor=user_id, owner=user_id,
        )
    return _adapt(inserted)


def update_saved_query(query_id: str, data: dict, user_id: str) -> Optional[dict]:
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

    params.append(query_id)
    params.append(user_id)
    with db.transaction() as transaction:
        existing = transaction.query_one(
            "SELECT id, created_by FROM saved_queries WHERE id = @param0 AND created_by = @param1 FOR UPDATE",
            [query_id, user_id],
        )
        if not existing:
            return None
        updated = transaction.query_one(
            f"UPDATE saved_queries SET {', '.join(updates)} "
            f"WHERE id = @param{i} AND created_by = @param{i + 1} "
            "RETURNING id, name, description, sql_text, created_by, created_at, modified_by, modified_at",
            params,
        )
        if not updated:
            raise RuntimeError("Saved query changed after acquiring its row lock")
        _enqueue(
            transaction, family="saved_queries", operation="update",
            record_id=str(query_id), payload=_product_document(updated),
            actor=user_id, owner=str(existing["created_by"]),
        )
    return _adapt(updated)


def delete_saved_query(query_id: str, user_id: str) -> bool:
    with db.transaction() as transaction:
        existing = transaction.query_one(
            "SELECT id, created_by FROM saved_queries WHERE id = @param0 AND created_by = @param1 FOR UPDATE",
            [query_id, user_id],
        )
        if not existing:
            return False
        deleted = transaction.execute(
            "DELETE FROM saved_queries WHERE id = @param0 AND created_by = @param1",
            [query_id, user_id],
        )
        if deleted != 1:
            raise RuntimeError("Saved query changed after acquiring its row lock")
        _enqueue(
            transaction, family="saved_queries", operation="delete",
            record_id=str(query_id), payload={"id": str(query_id), "deleted": True},
            actor=user_id, owner=str(existing["created_by"]),
        )
    return True
