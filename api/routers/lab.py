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
from services.sql_guard import PLATFORM_METADATA_TABLES, assert_no_platform_tables
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


# Platform's own tables (PLATFORM_METADATA_TABLES) live in services.sql_guard, shared
# with the query guard so the listing filter and the SQL guard can never drift apart.
# They live in the metadata database (which currently also holds the demo/open-source
# data) and must never appear in — or be queryable from — SQL Lab.


def _hide_platform_tables(tables: list, resolved_db: str) -> list:
    """Drop platform metadata tables from a Lab table listing, but only for the
    metadata database — a dedicated data source is returned untouched."""
    if resolved_db != settings.METADATA_DATABASE:
        return tables
    return [
        t for t in tables
        if str((t or {}).get("name", "")).lower() not in PLATFORM_METADATA_TABLES
    ]


def _engine_source(source_id: str) -> dict:
    """Resolve a browser-selected ID to one active native catalog only."""
    source = meta_db.query_one(
        "SELECT id, name, engine_catalog FROM catalog_sources "
        "WHERE id = @param0 AND lifecycle = 'active' AND adapter_type = 'native'",
        [source_id],
    )
    if not source or not source.get("engine_catalog"):
        raise HTTPException(404, "Active Engine catalog source not found")
    return source


def _engine_tokens(sql: str) -> list[str]:
    """Small SQL lexer for statement count and three-part catalog references.

    It intentionally does not interpret SQL. It skips strings/comments so a
    catalog-looking value or semicolon in a literal cannot affect the scope gate.
    """
    tokens, index, size = [], 0, len(sql)
    while index < size:
        char = sql[index]
        if char.isspace():
            index += 1
        elif sql.startswith("--", index):
            newline = sql.find("\n", index + 2)
            index = size if newline < 0 else newline + 1
        elif sql.startswith("/*", index):
            end = sql.find("*/", index + 2)
            if end < 0:
                raise HTTPException(400, "Engine SQL contains an unterminated comment")
            index = end + 2
        elif char == "'":
            index += 1
            closed = False
            while index < size:
                if sql[index] == "'":
                    if index + 1 < size and sql[index + 1] == "'":
                        index += 2
                    else:
                        index += 1
                        closed = True
                        break
                else:
                    index += 1
            if not closed:
                raise HTTPException(400, "Engine SQL contains an unterminated string")
        elif char == "$":
            # PostgreSQL/Trino-style dollar strings may contain semicolons,
            # comments, and identifier-looking text. Treat the whole body as a
            # literal so it cannot change the one-statement or catalog scope.
            marker_end = sql.find("$", index + 1)
            marker = sql[index:marker_end + 1] if marker_end >= 0 else ""
            tag = marker[1:-1]
            if marker and (not tag or (tag[0].isalpha() or tag[0] == "_") and all(c.isalnum() or c == "_" for c in tag)):
                end = sql.find(marker, marker_end + 1)
                if end < 0:
                    raise HTTPException(400, "Engine SQL contains an unterminated dollar string")
                index = end + len(marker)
            else:
                index += 1
        elif char in ('"', '`'):
            quote, value = char, []
            index += 1
            while index < size:
                if sql[index] == quote:
                    if index + 1 < size and sql[index + 1] == quote:
                        value.append(quote)
                        index += 2
                    else:
                        index += 1
                        break
                else:
                    value.append(sql[index])
                    index += 1
            else:
                raise HTTPException(400, "Engine SQL contains an unterminated identifier")
            tokens.append("".join(value))
        elif char.isalnum() or char == '_':
            start = index
            index += 1
            while index < size and (sql[index].isalnum() or sql[index] in "_$"):
                index += 1
            tokens.append(sql[start:index])
        elif char in '.;':
            tokens.append(char)
            index += 1
        else:
            index += 1
    return tokens


def _engine_query(sql: str, catalog: str) -> str:
    """Keep Lab Engine execution read-only and scoped to the selected catalog."""
    tokens = _engine_tokens(sql)
    semicolons = [index for index, token in enumerate(tokens) if token == ';']
    if len(semicolons) > 1 or (semicolons and semicolons[0] != len(tokens) - 1):
        raise HTTPException(400, "Engine SQL Lab accepts one statement")
    if semicolons:
        tokens.pop()
    if not tokens or tokens[0].lower() not in {"select", "with"}:
        raise HTTPException(400, "Engine SQL Lab accepts one SELECT or WITH statement")
    for index in range(len(tokens) - 4):
        if tokens[index + 1] == '.' and tokens[index + 3] == '.' and tokens[index].casefold() != catalog.casefold():
            raise HTTPException(403, "Engine query references a catalog outside the selected source")
    return sql.strip().rstrip(';').rstrip()


@router.get("/lab/engine/sources")
def list_engine_sources(response: Response, ctx=Depends(require_min_role("Viewer"))):
    response.headers.update(NO_CACHE)
    rows = meta_db.query(
        "SELECT id, name, engine_catalog FROM catalog_sources "
        "WHERE lifecycle = 'active' AND adapter_type = 'native' ORDER BY name"
    ).get("rows") or []
    return {"success": True, "sources": [
        {"id": row["id"], "name": row["name"], "catalog": row["engine_catalog"]} for row in rows
    ]}


@router.get("/lab/engine/{source_id}/schemas")
def list_engine_schemas(source_id: str, response: Response, ctx=Depends(require_min_role("Viewer"))):
    from services import engine_bridge
    response.headers.update(NO_CACHE)
    source = _engine_source(source_id)
    result = engine_bridge.schemas(source["engine_catalog"], ctx.email, ctx.role) or {}
    return {"success": True, "schemas": result.get("schemas") or []}


@router.get("/lab/engine/{source_id}/schemas/{schema}/tables")
def list_engine_tables(source_id: str, schema: str, response: Response, ctx=Depends(require_min_role("Viewer"))):
    from services import engine_bridge
    response.headers.update(NO_CACHE)
    source = _engine_source(source_id)
    result = engine_bridge.tables(source["engine_catalog"], schema, ctx.email, ctx.role) or {}
    return {"success": True, "tables": result.get("tables") or []}


@router.get("/lab/engine/{source_id}/schemas/{schema}/tables/{table}/columns")
def get_engine_table_columns(source_id: str, schema: str, table: str, response: Response,
                             ctx=Depends(require_min_role("Viewer"))):
    from services import engine_bridge
    response.headers.update(NO_CACHE)
    source = _engine_source(source_id)
    raw_columns = engine_bridge.table_columns(source["engine_catalog"], schema, table, ctx.email, ctx.role)
    columns = []
    for column in raw_columns:
        if not isinstance(column, dict) or not isinstance(column.get("name"), str) or "data_type" not in column:
            raise HTTPException(502, "Engine table definition contains an invalid column")
        columns.append({
            "name": column["name"],
            "dataType": str(column["data_type"]),
            "isNullable": bool(column.get("nullable", True)),
        })
    return {"success": True, "schema": {"columns": columns}}


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
    resolved = _resolve_db(data.database)
    assert_no_platform_tables(data.sql, resolved)
    result = pool.execute_query(data.sql, resolved)
    return {"columns": result.get("columns", []), "rows": result.get("rows", []),
            "rowCount": result.get("row_count", 0)}


@router.post("/lab/query")
async def run_query(request: Request, data: LabQueryBody, ctx=Depends(require_min_role("Analyst"))):
    user = ctx.email
    sql_execute_limiter.check(user)
    user_id = user
    sql = data.query
    database = _resolve_db(data.database)
    engine_source_id = data.engineSourceId
    engine_schema = data.engineSchema
    if engine_source_id:
        from services import engine_bridge
        source = _engine_source(engine_source_id)
        scoped_sql = _engine_query(sql, source["engine_catalog"])
        start_time = int(time.time() * 1000)
        result = await asyncio.to_thread(
            engine_bridge.execute,
            scoped_sql, source["engine_catalog"], user, ctx.role, engine_schema,
        )
        duration_ms = int(time.time() * 1000) - start_time
        rows = result.get("data", [])
        columns = [
            column.get("name", "") if isinstance(column, dict) else str(column)
            for column in result.get("columns", [])
        ]
        try:
            history_svc.create_history({
                "sql_text": scoped_sql, "duration_ms": duration_ms,
                "database_name": "engine:" + source["engine_catalog"],
                "row_count": len(rows), "status": "success",
                "dataset_id": int(data.datasetId) if data.datasetId else None,
                "trigger_source": "lab", "run_context": None, "tables_used": None,
                "started_at": start_time,
            }, user)
        except Exception as error:
            print(f"[History] Failed to record Engine lab query: {error}")
        return {
            "success": True,
            "columns": columns,
            "rows": rows,
            "rowCount": len(rows),
            "executionTime": duration_ms / 1000,
        }
    assert_no_platform_tables(sql, database)
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
    # Never let CTAS read from or write into the control plane.
    assert_no_platform_tables(source_sql, data.database)
    if data.table_name.strip().lower() in PLATFORM_METADATA_TABLES and _resolve_db(data.database) == settings.METADATA_DATABASE:
        raise HTTPException(status_code=403, detail="Cannot create a table with a reserved platform name.")
    # Bracket-escape the identifiers (validated in CtasBody, but quote defensively).
    target = f"{quote_identifier(data.schema)}.{quote_identifier(data.table_name)}"
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

    assert_no_platform_tables(f"{schema}.{table}", database)
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
