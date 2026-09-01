"""Theme service — port of theme.service.ts."""

import re
import time
import database.metadata as db

DEFAULT_COLOR = "#8f9192"
_CACHE_TTL = 300  # seconds

_cache: dict[str, tuple[str, float]] = {}  # email → (color, expires_at)


def get_user_theme(user_email: str) -> dict:
    entry = _cache.get(user_email)
    if entry and time.monotonic() < entry[1]:
        return {"theme_color": entry[0]}
    try:
        result = db.query_one(
            "SELECT theme_color FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
        )
        color = result["theme_color"] if result else DEFAULT_COLOR
    except Exception:
        color = DEFAULT_COLOR
    _cache[user_email] = (color, time.monotonic() + _CACHE_TTL)
    return {"theme_color": color}


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
    _cache.pop(user_email, None)


def delete_user_theme(user_email: str) -> None:
    try:
        db.execute(
            "DELETE FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
        )
    except Exception:
        pass  # setup mode — no metadata DB yet, ignore silently
    _cache.pop(user_email, None)
