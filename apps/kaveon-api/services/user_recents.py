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
