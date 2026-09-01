"""Shared SQL-Lab guard — keep user-run SQL away from the control plane.

Hiding platform tables from the Lab table listing is cosmetic: a user can still
reference them by name. These guards reject dangerous user-supplied SQL and close:

  - Viewer/Analyst reading `auth_config` / `local_users` (secret + hash exposure)
  - Analyst UPDATE / DELETE / DROP against Kaveon's own metadata tables
  - a sub-Analyst (Viewer) using the dashboard-source exception to run non-SELECT
    or stacked statements

A registered data source (a user's own Postgres / Fabric / MySQL DB) is never
affected — the platform-table guard only fires against the metadata database.
"""

import re
from typing import Optional

from fastapi import HTTPException

from config import settings

# Kaveon's own control-plane tables. Must never be readable/writable from SQL Lab.
PLATFORM_METADATA_TABLES = frozenset({
    "activity", "auth_config", "charts", "context_answer_cache", "context_snapshots",
    "dashboards", "data_sources", "dataset_columns", "dataset_dimensions",
    "dataset_metrics", "datasets", "dlm_answers", "dlm_artifact", "dlm_router",
    "dlm_sketch", "dlm_value_index", "favorites", "local_users", "query_history",
    "saved_queries", "user_recents", "user_themes",
})

# Word-boundary match catches bare, bracketed ("[t]"), quoted ("t") and
# schema-qualified (dbo.t) references alike — all have a non-word char at the edge.
_PLATFORM_TABLE_RE = re.compile(
    r"\b(" + "|".join(re.escape(t) for t in PLATFORM_METADATA_TABLES) + r")\b",
    re.IGNORECASE,
)

# A platform name that is an OUTPUT ALIAS (… AS "Charts") is never a table read,
# so it must not trip the guard. This lets a dataset legitimately have a metric
# labelled "Charts" or a column like charts_created (already excluded by \b).
_ALIAS_PREFIX_RE = re.compile(r"\bas\s+[\"\[`]?\s*$", re.IGNORECASE)

# A read-only statement starts with SELECT or a WITH … SELECT CTE.
_READONLY_RE = re.compile(r"^\s*(?:select|with)\b", re.IGNORECASE)


def is_metadata_db(database: Optional[str]) -> bool:
    """True when `database` names the metadata database (where the control plane lives)."""
    md = (settings.METADATA_DATABASE or "").strip().lower()
    return bool(md) and (database or "").strip().lower() == md


def assert_no_platform_tables(sql: str, database: Optional[str]) -> None:
    """Raise 403 if `sql` references a platform table while targeting the metadata DB."""
    if not is_metadata_db(database):
        return
    text = sql or ""
    for m in _PLATFORM_TABLE_RE.finditer(text):
        # Skip output aliases (e.g. SUM(charts_created) AS "Charts") — not a table read.
        if _ALIAS_PREFIX_RE.search(text[:m.start()]):
            continue
        raise HTTPException(
            status_code=403,
            detail={
                "code": "forbidden_table",
                "message": f"Access to platform table '{m.group(1).lower()}' is not permitted from SQL Lab.",
            },
        )


def is_read_only(sql: str) -> bool:
    """True only for a single SELECT/WITH statement — no writes, no stacked statements."""
    s = (sql or "").strip().rstrip(";").strip()
    if ";" in s:                      # reject stacked statements (SELECT 1; DROP …)
        return False
    return bool(_READONLY_RE.match(s))


def assert_read_only(sql: str) -> None:
    """Raise 403 unless `sql` is a single read-only statement."""
    if not is_read_only(sql):
        raise HTTPException(
            status_code=403,
            detail={
                "code": "forbidden",
                "message": "Only read-only (SELECT) queries are permitted for your role.",
            },
        )
