"""User recents service."""

from typing import List
import logging
import database.metadata as db
from services import product_outbox
from services.user_recent_backfill import canonical, normalize_item_id, record_id

MAX_CROSS_OWNER_DELETE = 100
def _document(row):
    created=row.get("created_at");created=created.isoformat() if hasattr(created,"isoformat") else str(created or "")
    return {"user_email":str(row["user_email"]),"item_id":normalize_item_id(row.get("item_id"),row.get("type")),"label":row.get("label"),"href":row.get("href"),"type":row.get("type"),"created_at":created}
def _event(tx,operation,row,actor):
    owner=str(row["user_email"]);item=str(row["item_id"])
    normalized=normalize_item_id(item,row.get("type"))
    product_outbox.enqueue(tx,family="user_recents",operation=operation,record_id=record_id(owner,normalized),payload={} if operation=="delete" else _document(row),actor=actor,owner=owner)


def get_recents(user_email: str) -> List[dict]:
    rows=db.query("""
        SELECT item_id, label, href, type, created_at
        FROM user_recents
        WHERE user_email = @param0
        ORDER BY created_at DESC
    """, [user_email])["rows"][:20]
    try:
        from services import product_shadow_read
        report=product_shadow_read.observe_user_recent_list(rows,user_email)
        if report.get("enabled"):logging.getLogger(__name__).info("user_recent_shadow %s",report)
    except Exception as error:logging.getLogger(__name__).warning("user_recent_shadow_error type=%s",type(error).__name__)
    return rows


def add_recent(user_email: str, item_id: str, label: str, href: str, item_type: str) -> None:
    # Postgres/MySQL don't support T-SQL MERGE; use the (user_email, item_id)
    # unique constraint for a native upsert. The dialect layer rewrites
    # GETUTCDATE()->NOW() and @paramN->%s.
    db_type = (__import__("os").environ.get("METADATA_DB_TYPE") or "").lower()
    if db_type == "mysql":
        db.execute("""
            INSERT INTO user_recents (user_email, item_id, label, href, type, created_at)
            VALUES (@param0, @param1, @param2, @param3, @param4, GETUTCDATE())
            ON DUPLICATE KEY UPDATE label = @param2, href = @param3, type = @param4, created_at = GETUTCDATE()
        """, [user_email, item_id, label, href, item_type])
    elif db_type == "postgresql":
        with db.transaction() as tx:
            tx.execute("SELECT pg_advisory_xact_lock(hashtext(@param0))",[user_email])
            current=tx.query_one("SELECT id FROM user_recents WHERE user_email=@param0 AND item_id=@param1 FOR UPDATE",[user_email,item_id])
            row=tx.query_one("""
            INSERT INTO user_recents (user_email, item_id, label, href, type, created_at)
            VALUES (@param0, @param1, @param2, @param3, @param4, GETUTCDATE())
            ON CONFLICT (user_email, item_id)
            DO UPDATE SET label = @param2, href = @param3, type = @param4, created_at = GETUTCDATE()
            RETURNING id,user_email,item_id,label,href,type,created_at
        """, [user_email, item_id, label, href, item_type])
            evicted=tx.query("SELECT id,user_email,item_id,label,href,type,created_at FROM user_recents WHERE user_email=@param0 ORDER BY created_at DESC,item_id OFFSET 20 LIMIT 2 FOR UPDATE",[user_email])["rows"]
            if len(evicted)>1:raise RuntimeError("user recent retention invariant is invalid")
            if evicted:tx.execute("DELETE FROM user_recents WHERE id=@param0",[evicted[0]["id"]])
            inserted_was_evicted=bool(evicted and evicted[0]["id"]==row["id"])
            if inserted_was_evicted:
                if current:_event(tx,"delete",row,user_email)
            else:
                _event(tx,"update" if current else "create",row,user_email)
                if evicted:_event(tx,"delete",evicted[0],user_email)
        return
    else:
        db.execute("""
            MERGE INTO user_recents AS target
            USING (SELECT @param0 AS user_email, @param1 AS item_id) AS source
                ON target.user_email = source.user_email AND target.item_id = source.item_id
            WHEN MATCHED THEN
                UPDATE SET label = @param2, href = @param3, type = @param4, created_at = GETUTCDATE()
            WHEN NOT MATCHED THEN
                INSERT (user_email, item_id, label, href, type, created_at)
                VALUES (@param0, @param1, @param2, @param3, @param4, GETUTCDATE());
        """, [user_email, item_id, label, href, item_type])
    # Trim to 20 most recent per user
    db_type = db_type or (__import__("os").environ.get("METADATA_DB_TYPE") or "").lower()
    if db_type in ("postgresql", "mysql"):
        db.execute("""
            DELETE FROM user_recents
            WHERE user_email = @param0
              AND id NOT IN (
                  SELECT id FROM user_recents
                  WHERE user_email = @param0
                  ORDER BY created_at DESC
                  LIMIT 20
              )
        """, [user_email, user_email])
    else:
        db.execute("""
            DELETE FROM user_recents
            WHERE user_email = @param0
              AND id NOT IN (
                  SELECT TOP 20 id FROM user_recents
                  WHERE user_email = @param0
                  ORDER BY created_at DESC
              )
        """, [user_email, user_email])


def remove_recent(user_email: str, item_id: str) -> None:
    with db.transaction() as tx:
        row=tx.query_one("SELECT id,user_email,item_id,type FROM user_recents WHERE user_email=@param0 AND item_id=@param1 FOR UPDATE",[user_email,item_id])
        if not row:return
        tx.execute("DELETE FROM user_recents WHERE id=@param0",[row["id"]]);_event(tx,"delete",row,user_email)


def clear_recents(user_email: str, item_type: str | None = None) -> int:
    """Clear a user's recents — all, or just one type."""
    with db.transaction() as tx:
        params=[user_email]+([item_type] if item_type else [])
        predicate="user_email=@param0"+(" AND type=@param1" if item_type else "")
        rows=tx.query(f"SELECT id,user_email,item_id,type FROM user_recents WHERE {predicate} ORDER BY item_id LIMIT 21 FOR UPDATE",params)["rows"]
        if len(rows)>20:raise RuntimeError("user recent owner retention invariant is invalid")
        for row in rows:
            tx.execute("DELETE FROM user_recents WHERE id=@param0",[row["id"]]);_event(tx,"delete",row,user_email)
        return len(rows)


def remove_recent_all_users(item_id: str, item_type: str) -> None:
    """Purge a recents entry for every user — call when the underlying
    dashboard/chart/dataset is deleted so it stops showing in anyone's recents."""
    with db.transaction() as tx:
        rows=tx.query("SELECT id,user_email,item_id,type FROM user_recents WHERE item_id=@param0 AND type=@param1 ORDER BY user_email LIMIT @param2 FOR UPDATE",[item_id,item_type,MAX_CROSS_OWNER_DELETE+1])["rows"]
        if len(rows)>MAX_CROSS_OWNER_DELETE:raise RuntimeError("cross-owner recent delete exceeds its fanout bound")
        for row in rows:
            tx.execute("DELETE FROM user_recents WHERE id=@param0",[row["id"]]);_event(tx,"delete",row,row["user_email"])
