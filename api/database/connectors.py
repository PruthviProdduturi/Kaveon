"""Execution boundary for registered, single-source SQL connectors.

The Engine catalog path is deliberately separate from this module. A
registered source is resolved by its KaveonDB source record, then the complete
statement is pushed to that one source through the existing connection pool.
Cross-source planning and joins are not implied by this boundary.
"""

from typing import Any, Dict, Optional

from . import pool


def execute_registered_source(
    sql: str,
    database: str,
    *,
    params: Optional[list] = None,
    timeout_seconds: Optional[float] = None,
    max_rows: Optional[int] = None,
    actor: Optional[str] = None,
    role: str = "Viewer",
) -> Dict[str, Any]:
    """Push one statement to one registered source.

    Source resolution, credentials, retries, statement bounds, and result
    shaping remain owned by :mod:`database.pool`; this function is the stable
    seam for future connector-specific pushdown without accidentally turning
    the API path into a cross-source executor.
    """

    return pool.execute_query(
        sql,
        database,
        params=params,
        timeout_seconds=timeout_seconds,
        max_rows=max_rows,
        actor=actor,
        role=role,
    )
