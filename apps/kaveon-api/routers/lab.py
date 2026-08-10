"""Lab router — /api/v1/lab."""

import asyncio
import json
import threading
from fastapi import APIRouter, Request, Response, HTTPException, Query, Depends
from middleware.auth import require_auth
from middleware.permissions import require_min_role
from middleware.rate_limit import sql_execute_limiter
from models.lab import SavedQueryCreate, SavedQueryUpdate, LabExecuteBody, LabQueryBody, SwitchDatabaseBody, CtasBody
import database.pool as pool
import database.metadata as meta_db
import services.saved_queries as saved_q_svc
import services.query_history as history_svc
from services.query_generator import quote_identifier
from config import settings
import time

router = APIRouter()

MAX_SQL_BYTES = 65_536
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}


@router.get("/lab/databases")
def list_databases(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    try:
        # Via the metadata adapter so is_active = 1 → TRUE on Postgres.
        result = meta_db.query(
            "SELECT database_name as [database], name as display_name, 0 as table_count "
            "FROM data_sources WHERE is_active = 1 ORDER BY name"
        )
        return {"success": True, "databases": result.get("rows") or []}
    except Exception:
        return {"success": True, "databases": []}


@router.get("/lab/saved-queries")
def list_saved_queries(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return saved_q_svc.list_saved_queries(user)


@router.get("/lab/saved-queries/{query_id}")
def get_saved_query(query_id: str, user: str = Depends(require_auth)):
    q = saved_q_svc.get_by_id(query_id, user)
    if not q:
        raise HTTPException(status_code=404, detail="Saved query not found")
    return q


@router.post("/lab/saved-queries", status_code=201)
def create_saved_query(data: SavedQueryCreate, user: str = Depends(require_auth)):
    return saved_q_svc.create_saved_query(data.model_dump(exclude_none=True), user)


@router.put("/lab/saved-queries/{query_id}")
def update_saved_query(query_id: str, data: SavedQueryUpdate, user: str = Depends(require_auth)):
    result = saved_q_svc.update_saved_query(query_id, data.model_dump(exclude_none=True), user)
    if not result:
        raise HTTPException(status_code=404, detail="Saved query not found")
    return result


@router.delete("/lab/saved-queries/{query_id}", status_code=204)
def delete_saved_query(query_id: str, user: str = Depends(require_auth)):
    deleted = saved_q_svc.delete_saved_query(query_id, user)
    if not deleted:
        raise HTTPException(status_code=404, detail="Saved query not found")


def _resolve_db(database: str | None) -> str:
    """Fall back to the metadata database (where the demo data lives) when the
    UI hasn't selected a data source yet — avoids a hard 'database required' fail."""
    return database or settings.METADATA_DATABASE


# Platform's own tables. These live in the metadata database (which currently
# also holds the demo/open-source data) and must never appear in SQL Lab — users
# browse *their* data, not Kaveon's control-plane. When the metadata store is
# split onto its own database, a pure data source simply won't match this filter.
PLATFORM_METADATA_TABLES = frozenset({
    "activity", "auth_config", "charts", "context_answer_cache", "context_snapshots",
    "dashboards", "data_sources", "dataset_columns", "dataset_dimensions",
    "dataset_metrics", "datasets", "favorites", "local_users", "query_history",
    "saved_queries", "user_recents", "user_themes",
})


def _hide_platform_tables(tables: list, resolved_db: str) -> list:
    """Drop platform metadata tables from a Lab table listing, but only for the
    metadata database — a dedicated data source is returned untouched."""
    if resolved_db != settings.METADATA_DATABASE:
        return tables
    return [
        t for t in tables
        if str((t or {}).get("name", "")).lower() not in PLATFORM_METADATA_TABLES
    ]


@router.get("/lab/tables")
def list_tables(response: Response, database: str = Query(default=None), user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    resolved = _resolve_db(database)
    tables = _hide_platform_tables(pool.get_tables(resolved), resolved)
    return {"success": True, "tables": tables}


@router.get("/lab/tables/{table_id}/columns")
def get_table_columns(table_id: str, database: str = Query(default=None), user: str = Depends(require_auth)):
    columns = pool.get_table_columns(table_id, _resolve_db(database))
    return columns


@router.get("/lab/schema/{schema}/{table_name}")
def get_schema(schema: str, table_name: str, database: str = Query(default=None), user: str = Depends(require_auth)):
    table_id = f"{schema}.{table_name}"
    columns = pool.get_table_columns(table_id, _resolve_db(database))
    return {"success": True, "schema": {"columns": columns}}


@router.post("/lab/execute")
def execute_sql(data: LabExecuteBody, response: Response, ctx=Depends(require_min_role("Analyst"))):
    user = ctx.email
    sql_execute_limiter.check(user)
    result = pool.execute_query(data.sql, _resolve_db(data.database))
    return {"columns": result.get("columns", []), "rows": result.get("rows", []),
            "rowCount": result.get("row_count", 0)}


@router.post("/lab/query")
async def run_query(request: Request, data: LabQueryBody, ctx=Depends(require_min_role("Analyst"))):
    user = ctx.email
    sql_execute_limiter.check(user)
    user_id = user
    sql = data.query
    database = _resolve_db(data.database)
    dataset_id = data.datasetId
    run_context = data.runContext
    tables_used = data.tablesUsed
    start_time = int(time.time() * 1000)

    trigger_source = "dataset-preview" if run_context == "dataset-detail" else "lab"

    # cancel_event is shared between the query thread and the disconnect watcher.
    # Setting it triggers cursor.cancel() inside execute_query_cancellable.
    cancel_event = threading.Event()

    loop = asyncio.get_event_loop()
    query_future = loop.run_in_executor(
        None,
        lambda: pool.execute_query_cancellable(sql, database, cancel_event=cancel_event),
    )

    # Poll for client disconnect every 500ms while the query runs.
    cancelled = False
    while not query_future.done():
        if await request.is_disconnected():
            cancel_event.set()
            cancelled = True
            break
        await asyncio.sleep(0.5)

    if cancelled:
        # Client disconnected — query has been signalled to cancel on the DB side.
        try:
            await query_future
        except Exception:
            pass
        return Response(status_code=204)

    try:
        result = await query_future
        duration_ms = int(time.time() * 1000) - start_time

        try:
            history_svc.create_history({
                "sql_text": sql, "duration_ms": duration_ms,
                "database_name": database,
                "row_count": result.get("row_count", 0), "status": "success",
                "dataset_id": int(dataset_id) if dataset_id else None,
                "trigger_source": trigger_source,
                "run_context": json.dumps({"context": run_context}) if run_context else None,
                "tables_used": json.dumps(tables_used) if tables_used else None,
                "started_at": start_time,
            }, user_id)
        except Exception as he:
            print(f"[History] Failed to record lab query: {he}")

        return {
            "success": True,
            "columns": result.get("columns", []),
            "rows": result.get("rows", []),
            "rowCount": result.get("row_count", 0),
            "executionTime": duration_ms / 1000,
        }
    except Exception as e:
        duration_ms = int(time.time() * 1000) - start_time
        try:
            history_svc.create_history({
                "sql_text": sql, "duration_ms": duration_ms,
                "database_name": database,
                "row_count": 0, "status": "error",
                "error_message": str(e),
                "dataset_id": int(dataset_id) if dataset_id else None,
                "trigger_source": trigger_source,
                "run_context": json.dumps({"context": run_context}) if run_context else None,
                "tables_used": json.dumps(tables_used) if tables_used else None,
                "started_at": start_time,
            }, user_id)
        except Exception as he:
            print(f"[History] Failed to record lab error: {he}")
        raise HTTPException(status_code=500, detail="Query execution failed")


@router.post("/lab/ctas", status_code=200)
def create_table_as_select(data: CtasBody, ctx=Depends(require_min_role("Analyst"))):
    user = ctx.email  # noqa: F841
    """Materialise a query as a new table using SELECT … INTO [schema].[table]."""
    sql_execute_limiter.check(user)
    # Wrap user SQL in SELECT … INTO — strip trailing semicolon first
    source_sql = data.sql.rstrip().rstrip(";").strip()
    target = f"[{data.schema}].[{data.table_name}]"
    ctas_sql = f"SELECT * INTO {target} FROM (\n{source_sql}\n) AS __ctas_source"
    try:
        pool.execute_query(ctas_sql, data.database)
        return {"success": True, "table": f"{data.schema}.{data.table_name}"}
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.post("/lab/switch-database")
def switch_database(data: SwitchDatabaseBody, user: str = Depends(require_auth)):
    return {"success": True, "database": data.database_name}


@router.get("/lab/query-history")
def get_query_history(response: Response, limit: int = Query(default=50), user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    # Scope to the requesting user — never leak other users' SQL history.
    return history_svc.list_history(user, limit)


@router.delete("/lab/query-history")
def clear_query_history(user: str = Depends(require_auth)):
    count = history_svc.delete_all_history(user)
    return {"deleted": count}


@router.post("/lab/record-query")
def record_query(user: str = Depends(require_auth)):
    """No-op — kept for backwards compatibility. History is written by /sql/execute."""
    return {"success": True}


@router.get("/lab/distinct/{schema}/{table}/{column}")
def get_distinct_values(
    schema: str, table: str, column: str,
    database: str = Query(...),
    limit: int = Query(default=100),
    user: str = Depends(require_auth),
):
    user_id = user
    start_time = int(time.time() * 1000)

    safe_schema = quote_identifier(schema)
    safe_table = quote_identifier(table)
    safe_column = quote_identifier(column)
    safe_limit = min(max(1, limit), 1000)

    sql = (
        f"SELECT DISTINCT TOP {safe_limit} {safe_column} as value "
        f"FROM {safe_schema}.{safe_table} "
        f"WHERE {safe_column} IS NOT NULL "
        f"ORDER BY {safe_column}"
    )

    try:
        result = pool.execute_query(sql, database)
        duration_ms = int(time.time() * 1000) - start_time

        values = []
        for row in (result.get("rows") or []):
            if isinstance(row, list):
                values.append(row[0])
            elif isinstance(row, dict):
                v = row.get("value")
                if v is None and row:
                    v = list(row.values())[0]
                values.append(v)
            else:
                values.append(row)
        values = [v for v in values if v is not None]

        try:
            history_svc.create_history({
                "sql_text": sql.strip(), "duration_ms": duration_ms,
                "database_name": database,
                "row_count": len(values), "status": "success",
                "trigger_source": "dataset-filter-values",
                "tables_used": json.dumps([f"{schema}.{table}"]),
                "run_context": json.dumps({"column": column, "database": database}),
                "started_at": start_time,
            }, user_id)
        except Exception as he:
            print(f"[History] Failed to record distinct values query: {he}")

        return {"success": True, "values": values}

    except Exception as e:
        duration_ms = int(time.time() * 1000) - start_time
        error_sql = f"SELECT DISTINCT TOP 100 {safe_column} as value FROM {safe_schema}.{safe_table} WHERE {safe_column} IS NOT NULL ORDER BY {safe_column}"
        try:
            history_svc.create_history({
                "sql_text": error_sql, "duration_ms": duration_ms,
                "database_name": database,
                "row_count": 0, "status": "error", "error_message": str(e),
                "trigger_source": "dataset-filter-values",
                "tables_used": json.dumps([f"{schema}.{table}"]),
                "started_at": start_time,
            }, user_id)
        except Exception as he:
            print(f"[History] Failed to record distinct values error: {he}")
        raise HTTPException(status_code=500, detail="Query execution failed")
