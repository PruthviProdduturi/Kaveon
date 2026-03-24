"""Theme service — port of theme.service.ts."""

import re
import database.metadata as db

DEFAULT_COLOR = "#8f9192"


def get_user_theme(user_email: str) -> dict:
    try:
        result = db.query_one(
            "SELECT theme_color FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
        )
        return {"theme_color": result["theme_color"] if result else DEFAULT_COLOR}
    except Exception:
        return {"theme_color": DEFAULT_COLOR}


def save_user_theme(user_email: str, theme_color: str) -> None:
    if not re.match(r"^#[0-9A-Fa-f]{6}$", theme_color):
        raise ValueError("Invalid hex color format. Expected format: #RRGGBB")

    try:
        db.execute(
        """
        MERGE INTO dbo.user_themes AS target
        USING (SELECT @param0 AS user_email, @param1 AS theme_color) AS source
        ON target.user_email = source.user_email
        WHEN MATCHED THEN
            UPDATE SET theme_color = source.theme_color
        WHEN NOT MATCHED THEN
            INSERT (user_email, theme_color) VALUES (source.user_email, source.theme_color);
        """,
        [user_email, theme_color],
        )
    except Exception:
        pass  # setup mode — no metadata DB yet, ignore silently


def delete_user_theme(user_email: str) -> None:
    try:
        db.execute(
            "DELETE FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
        )
    except Exception:
        pass  # setup mode — no metadata DB yet, ignore silently
