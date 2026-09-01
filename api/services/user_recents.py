"""User recents service."""

from typing import List
import database.metadata as db


def get_recents(user_email: str) -> List[dict]:
    return db.query("""
        SELECT item_id, label, href, type, created_at
        FROM user_recents
        WHERE user_email = @param0
        ORDER BY created_at DESC
    """, [user_email])["rows"][:20]


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
        db.execute("""
            INSERT INTO user_recents (user_email, item_id, label, href, type, created_at)
            VALUES (@param0, @param1, @param2, @param3, @param4, GETUTCDATE())
            ON CONFLICT (user_email, item_id)
            DO UPDATE SET label = @param2, href = @param3, type = @param4, created_at = GETUTCDATE()
        """, [user_email, item_id, label, href, item_type])
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
    db.execute("""
        DELETE FROM user_recents WHERE user_email = @param0 AND item_id = @param1
    """, [user_email, item_id])


def clear_recents(user_email: str, item_type: str | None = None) -> int:
    """Clear a user's recents — all, or just one type."""
    if item_type:
        return db.execute(
            "DELETE FROM user_recents WHERE user_email = @param0 AND type = @param1",
            [user_email, item_type],
        )
    return db.execute(
        "DELETE FROM user_recents WHERE user_email = @param0", [user_email]
    )


def remove_recent_all_users(item_id: str, item_type: str) -> None:
    """Purge a recents entry for every user — call when the underlying
    dashboard/chart/dataset is deleted so it stops showing in anyone's recents."""
    db.execute("""
        DELETE FROM user_recents WHERE item_id = @param0 AND type = @param1
    """, [item_id, item_type])
