"""Theme service — port of theme.service.ts."""

import re
import time
import logging
import database.metadata as db
from services import product_outbox, product_shadow_read

DEFAULT_COLOR = "#8f9192"
_CACHE_TTL = 300  # seconds

_cache: dict[str, tuple[str, float]] = {}  # email → (color, expires_at)


def _observe(result: dict, user_email: str) -> None:
    try:
        report = product_shadow_read.observe_user_theme(result, user_email)
        if report.get("enabled"):
            logging.getLogger(__name__).info("user_theme_shadow_read %s", report)
    except Exception as error:
        logging.getLogger(__name__).warning("user_theme_shadow_read_error type=%s", type(error).__name__)


def get_user_theme(user_email: str) -> dict:
    entry = _cache.get(user_email)
    if entry and time.monotonic() < entry[1]:
        result = {"theme_color": entry[0]}
        _observe(result, user_email)
        return result
    try:
        result = db.query_one(
            "SELECT theme_color FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
        )
        color = result["theme_color"] if result else DEFAULT_COLOR
    except Exception:
        color = DEFAULT_COLOR
    _cache[user_email] = (color, time.monotonic() + _CACHE_TTL)
    result = {"theme_color": color}
    _observe(result, user_email)
    return result


def save_user_theme(user_email: str, theme_color: str) -> None:
    if not re.match(r"^#[0-9A-Fa-f]{6}$", theme_color):
        raise ValueError("Invalid hex color format. Expected format: #RRGGBB")

    document = {"user_email": user_email, "theme_color": theme_color.lower()}
    with db.transaction() as transaction:
        current = transaction.query_one(
            "SELECT theme_color FROM dbo.user_themes WHERE user_email = @param0 FOR UPDATE",
            [user_email],
        )
        transaction.execute(
        """
        MERGE INTO dbo.user_themes AS target
        USING (SELECT @param0 AS user_email, @param1 AS theme_color) AS source
        ON target.user_email = source.user_email
        WHEN MATCHED THEN
            UPDATE SET theme_color = source.theme_color
        WHEN NOT MATCHED THEN
            INSERT (user_email, theme_color) VALUES (source.user_email, source.theme_color);
        """,
        [user_email, document["theme_color"]],
        )
        product_outbox.enqueue(transaction, family="user_themes",
                               operation="update" if current else "create",
                               record_id=user_email, payload=document,
                               actor=user_email, owner=user_email)
    _cache.pop(user_email, None)


def delete_user_theme(user_email: str) -> None:
    with db.transaction() as transaction:
        current = transaction.query_one(
            "SELECT theme_color FROM dbo.user_themes WHERE user_email = @param0 FOR UPDATE",
            [user_email],
        )
        if current:
            transaction.execute(
            "DELETE FROM dbo.user_themes WHERE user_email = @param0",
            [user_email],
            )
            product_outbox.enqueue(transaction, family="user_themes", operation="delete",
                                   record_id=user_email, payload={}, actor=user_email,
                                   owner=user_email)
    _cache.pop(user_email, None)
