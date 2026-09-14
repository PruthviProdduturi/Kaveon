"""Fail-closed write fence for PostgreSQL authority tables during cutover."""

import os
import re

from services.postgresql_retirement_gate import AUTHORITY_FAMILIES


ENVIRONMENT_KEY = "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED"
_WRITE_VERBS = frozenset({"insert", "update", "delete", "merge", "truncate", "alter", "drop", "create"})
_AUTHORITY_TABLES = frozenset(
    table.lower() for tables in AUTHORITY_FAMILIES.values() for table in tables
)


class PostgreSQLWriteFencedError(RuntimeError):
    """Raised before a fenced authority mutation reaches PostgreSQL."""


def enabled() -> bool:
    """Return the explicit fence state; only the exact value ``true`` enables it."""
    return os.getenv(ENVIRONMENT_KEY, "").strip().lower() == "true"


def _tokens(sql: str) -> list[str]:
    # Remove comments and string literals before inspecting identifiers. This
    # prevents a comment or payload value from being mistaken for a mutation.
    scrubbed = re.sub(r"/\*.*?\*/", " ", sql, flags=re.DOTALL)
    scrubbed = re.sub(r"--[^\r\n]*", " ", scrubbed)
    scrubbed = re.sub(r"'(?:''|[^'])*'", " ", scrubbed)
    return re.findall(r"[A-Za-z_][A-Za-z0-9_]*", scrubbed.lower())


def assert_allowed(sql: str, db_type: str) -> None:
    """Reject authority-table mutations when the PostgreSQL cutover fence is set."""
    if db_type != "postgresql" or not enabled():
        return
    tokens = _tokens(sql)
    if not tokens or not (_WRITE_VERBS & set(tokens)):
        return
    touched = sorted(_AUTHORITY_TABLES & set(tokens))
    if touched:
        raise PostgreSQLWriteFencedError(
            "PostgreSQL authority writes are fenced during KaveonDB cutover "
            f"(table: {touched[0]})"
        )
