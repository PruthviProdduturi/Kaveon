"""SQL router — /api/v1/sql."""

import hashlib
import json
import threading
import time
import uuid
from fastapi import APIRouter, BackgroundTasks, Request, Response, HTTPException, Query, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role
from middleware.rate_limit import sql_execute_limiter
from models.sql import SqlGenerateBody, SqlExecuteBody
import services.datasets as datasets_svc
import services.query_history as history_svc
from services.query_generator import build_chart_preview_query, build_distinct_filter_values_query
from services.sql_table_extractor import extract_tables_from_sql
import database.pool as pool

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}

# ── Query result cache ──────────────────────────────────────────────────────
# Keyed by SHA-256(database + sql_text). Entries expire after their TTL.
_QUERY_CACHE: dict[str, dict] = {}
_QUERY_CACHE_LOCK = threading.Lock()

def _cache_key(database: str, sql_text: str) -> str:
    return hashlib.sha256(f"{database}\x00{sql_text}".encode()).hexdigest()

def _cache_get(key: str, ttl: int) -> dict | None:
    with _QUERY_CACHE_LOCK:
        entry = _QUERY_CACHE.get(key)
    if entry and (time.time() - entry["ts"]) < ttl:
        return entry["result"]
    return None

def _cache_set(key: str, result: dict) -> None:
    with _QUERY_CACHE_LOCK:
        # Evict stale entries if cache exceeds 500 items
        if len(_QUERY_CACHE) >= 500:
            now = time.time()
            stale = [k for k, v in list(_QUERY_CACHE.items()) if now - v["ts"] > 3600]
            for k in stale[:200]:
                _QUERY_CACHE.pop(k, None)
        _QUERY_CACHE[key] = {"result": result, "ts": time.time()}

# ── Async job store ─────────────────────────────────────────────────────────
_ASYNC_JOBS: dict[str, dict] = {}
_ASYNC_JOBS_LOCK = threading.Lock()

def _async_job_run(job_id: str, data: SqlExecuteBody, user_id: str) -> None:
    start_time = int(time.time() * 1000)
    try:
        result = pool.execute_query(data.sql_text, data.database)
        duration_ms = int(time.time() * 1000) - start_time
        resolved_tables = data.tables_used or json.dumps(
            extract_tables_from_sql(data.sql_text)
        )
        try:
            history_svc.create_history({
                "sql_text": data.sql_text, "duration_ms": duration_ms,
                "row_count": result.get("row_count") or 0, "status": "success",
                "trigger_source": canonical_source(data.source or ""),
                "tables_used": resolved_tables, "run_context": "{}",
                "dataset_id": data.dataset_id,
                "started_at": start_time,
            }, user_id)
        except Exception:
            pass
        with _ASYNC_JOBS_LOCK:
            _ASYNC_JOBS[job_id] = {
                "status": "success",
                "columns": result.get("columns") or [],
                "rows": result.get("rows") or [],
                "duration_ms": duration_ms,
            }
    except Exception as e:
        duration_ms = int(time.time() * 1000) - start_time
        with _ASYNC_JOBS_LOCK:
            _ASYNC_JOBS[job_id] = {"status": "error", "error": str(e), "duration_ms": duration_ms}

_CANONICAL_SOURCE = {
    "dashboard": "dashboard-chart",
    "dashboard-chart": "dashboard-chart",
    "chart-builder": "chart-builder",
    "chart-builder-filter": "chart-builder-filter",
    "dashboard-filter": "dashboard-filter",
    "dataset-preview": "dataset-preview",
    "dataset-filter": "dataset-filter",
    "lab": "lab",
}


def canonical_source(raw: str) -> str:
    return _CANONICAL_SOURCE.get((raw or "").lower(), raw or "chart-builder")


@router.post("/sql/generate")
def generate_sql(data: SqlGenerateBody, ctx=Depends(require_min_role("Analyst"))):
    user = ctx.email
    dataset_id = data.dataset_id
    chart_type = data.chart_type
    config = data.config

    dataset = datasets_svc.get_dataset_by_id(str(dataset_id))
    if not dataset:
        raise HTTPException(status_code=400, detail="Dataset not found")

    dimensions = [
        {
            "table": dim.get("dimension_table") or dim.get("table_name"),
            "factKey": dim.get("fact_key"),
            "dimKey": dim.get("join_key"),
            "semanticColumns": [],
        }
        for dim in (dataset.get("dimensions") or [])
    ]

    table_name = dataset.get("table_name") or ""
    datasource = (
        f"{dataset['schema_name']}.{table_name}"
        if dataset.get("schema_name") and table_name
        else table_name
    )

    params = {
        "datasource": datasource,
        "sql_text": dataset.get("sql_text") or "",
        **config,
        "dimensions": dimensions,
        "columns": dataset.get("columns") or [],
        "database_name": dataset.get("database_name"),
    }

    sql_text = build_chart_preview_query(params)
    if not sql_text:
        raise HTTPException(status_code=400, detail="Failed to generate SQL from configuration")

    tables_used = [dataset.get("table_name") or datasource]
    for d in dimensions:
        if d.get("table") and d["table"] not in tables_used:
            tables_used.append(d["table"])

    return {"sql_text": sql_text, "tables_used": tables_used, "warnings": []}


@router.get("/sql/distinct-filter-values")
def distinct_filter_values(
    response: Response,
    dataset_id: str = Query(...), column: str = Query(...),
    fact_key: str = Query(default=None),
    limit: int = Query(default=100),
    source: str = Query(default=None),
    chart_id: str = Query(default=None),
    dashboard_id: str = Query(default=None),
    user: str = Depends(require_auth),
):
    response.headers.update(NO_CACHE)
    user_id = user
    row_limit = min(limit, 500)

    dataset = datasets_svc.get_dataset_by_id(dataset_id)
    if not dataset:
        raise HTTPException(status_code=400, detail="Dataset not found")
    if not dataset.get("table_name"):
        raise HTTPException(status_code=400, detail="Dataset is missing table_name")
    if not dataset.get("database_name"):
        raise HTTPException(status_code=400, detail="Dataset database_name is missing")

    datasource = (
        f"{dataset['schema_name']}.{dataset['table_name']}"
        if dataset.get("schema_name")
        else dataset["table_name"]
    )

    hint_fk = fact_key.lower() if fact_key else None
    raw_dims = [
        {"table": dim.get("dimension_table") or dim.get("table_name"),
         "factKey": dim.get("fact_key"), "dimKey": dim.get("join_key")}
        for dim in (dataset.get("dimensions") or [])
    ]
    if hint_fk:
        raw_dims.sort(key=lambda d: 0 if (d.get("factKey") or "").lower() == hint_fk else 1)

    query_result = build_distinct_filter_values_query({
        "datasource": datasource, "column": column,
        "dimensions": raw_dims, "columns": dataset.get("columns") or [],
        "limit": row_limit,
    })
    if not query_result:
        raise HTTPException(status_code=400, detail="Failed to generate distinct values query")

    sql_text = query_result["sql"]
    key_column = query_result["keyColumn"]
    filtering_tier = query_result["filteringTier"]

    filter_trigger = canonical_source(source or "chart-builder-filter") or "chart-builder-filter"
    tables_used_list = [datasource]
    for dim in (dataset.get("dimensions") or []):
        dt = dim.get("dimension_table") or dim.get("table_name")
        if dt and dt not in tables_used_list:
            tables_used_list.append(dt)

    run_context = json.dumps({
        "source": filter_trigger, "datasetId": int(dataset_id), "column": column,
        "filteringTier": filtering_tier, "database": dataset["database_name"],
        **({} if not chart_id else {"chartId": int(chart_id)}),
        **({} if not dashboard_id else {"dashboardId": dashboard_id}),
    })

    start_time = int(time.time() * 1000)
    try:
        result = pool.execute_query(sql_text, dataset["database_name"])
    except Exception as e:
        duration_ms = int(time.time() * 1000) - start_time
        try:
            history_svc.create_history({
                "sql_text": sql_text, "duration_ms": duration_ms,
                "row_count": 0, "status": "error", "error_message": str(e),
                "trigger_source": filter_trigger,
                "tables_used": json.dumps(tables_used_list),
                "run_context": run_context, "dataset_id": int(dataset_id),
                "started_at": start_time,
            }, user_id)
        except Exception:
            pass
        raise HTTPException(status_code=500, detail="Query execution failed")

    duration_ms = int(time.time() * 1000) - start_time
    try:
        history_svc.create_history({
            "sql_text": sql_text, "duration_ms": duration_ms,
            "row_count": len(result.get("rows") or []), "status": "success",
            "trigger_source": filter_trigger,
            "tables_used": json.dumps(tables_used_list),
            "run_context": run_context, "dataset_id": int(dataset_id),
            "started_at": start_time,
        }, user_id)
    except Exception:
        pass

    rows = result.get("rows_objects") or result.get("rows") or []
    values = []
    for row in rows:
        if isinstance(row, list):
            values.append({"key": row[0], "value": row[1] if len(row) > 1 else row[0]})
        elif isinstance(row, dict):
            values.append({"key": row.get("key", row.get("value")), "value": row.get("value", row.get("key"))})
        else:
            values.append({"key": row, "value": row})

    return {"success": True, "values": values, "keyColumn": key_column, "filteringTier": filtering_tier}


@router.post("/sql/execute")
def execute_sql(data: SqlExecuteBody, response: Response, ctx: UserContext = Depends(require_user_context)):
    # Viewers may only execute from dashboard/filter context — not from builder or lab
    _dashboard_sources = {"dashboard-chart", "dashboard-filter", "dataset-filter", "dataset-preview"}
    from middleware.permissions import ROLE_LEVELS
    if ROLE_LEVELS.get(ctx.role, 0) < ROLE_LEVELS["Analyst"]:
        src = (data.source or "").lower()
        if src not in _dashboard_sources:
            from fastapi import HTTPException as _HTTPException
            raise _HTTPException(
                status_code=403,
                detail={"code": "forbidden", "message": "Analyst role required to execute queries."},
            )
    sql_execute_limiter.check(ctx.email)
    response.headers.update(NO_CACHE)
    user_id = ctx.email

    sql_text = data.sql_text
    database = data.database
    source = data.source
    tables_used = data.tables_used
    chart_id = data.chart_id
    dashboard_id = data.dashboard_id
    chart_type = data.chart_type
    dataset_id = data.dataset_id
    row_limit = data.row_limit
    use_cache = data.use_cache or False
    cache_ttl = data.cache_ttl or 300

    trigger_source = canonical_source(source)
    resolved_tables = tables_used or json.dumps(extract_tables_from_sql(sql_text))
    run_context = json.dumps({
        "source": trigger_source, "database": database,
        **({} if chart_id is None else {"chartId": int(chart_id)}),
        **({} if dashboard_id is None else {"dashboardId": dashboard_id}),
        **({} if chart_type is None else {"chartType": chart_type}),
        **({} if dataset_id is None else {"datasetId": int(dataset_id)}),
    })

    # Return cached result immediately when available (dashboard-chart queries)
    ck = _cache_key(database, sql_text)
    if use_cache:
        cached = _cache_get(ck, cache_ttl)
        if cached is not None:
            row_count = cached.get("row_count") or 0
            return {
                "columns": cached.get("columns") or [],
                "rows": cached.get("rows") or [],
                "message": f"Returned {row_count} rows (cached)" if row_count else None,
                "duration_ms": 0,
                "from_cache": True,
            }

    start_time = int(time.time() * 1000)
    try:
        result = pool.execute_query(sql_text, database)

        if row_limit:
            lim = min(max(1, int(row_limit)), 5000)
            rows = result.get("rows") or []
            if len(rows) > lim:
                result["rows"] = rows[:lim]
                result["row_count"] = lim
    except Exception as e:
        duration_ms = int(time.time() * 1000) - start_time
        try:
            history_svc.create_history({
                "sql_text": sql_text, "duration_ms": duration_ms,
                "row_count": 0, "status": "error", "error_message": str(e),
                "trigger_source": trigger_source, "tables_used": resolved_tables,
                "run_context": run_context,
                "dataset_id": int(dataset_id) if dataset_id is not None else None,
                "started_at": start_time,
            }, user_id)
        except Exception as he:
            print(f"[History] Failed to save error history: {he}")
        raise HTTPException(status_code=500, detail="Query execution failed")

    duration_ms = int(time.time() * 1000) - start_time

    if use_cache:
        _cache_set(ck, {"columns": result.get("columns") or [], "rows": result.get("rows") or [], "row_count": result.get("row_count") or 0})

    try:
        history_svc.create_history({
            "sql_text": sql_text, "duration_ms": duration_ms,
            "row_count": result.get("row_count") or 0, "status": "success",
            "trigger_source": trigger_source, "tables_used": resolved_tables,
            "run_context": run_context,
            "dataset_id": int(dataset_id) if dataset_id is not None else None,
            "started_at": start_time,
        }, user_id)
    except Exception as e:
        print(f"[History] Failed to save execute history: {e}")

    row_count = result.get("row_count") or 0
    return {
        "columns": result.get("columns") or [],
        "rows": result.get("rows") or [],
        "message": f"Returned {row_count} rows" if row_count else None,
        "duration_ms": duration_ms,
        "from_cache": False,
    }


@router.post("/sql/execute-async")
def execute_sql_async(
    data: SqlExecuteBody,
    background_tasks: BackgroundTasks,
    ctx=Depends(require_min_role("Analyst")),
):
    """Start an async query job. Returns job_id immediately for polling."""
    user = ctx.email
    sql_execute_limiter.check(user)
    job_id = str(uuid.uuid4())
    with _ASYNC_JOBS_LOCK:
        _ASYNC_JOBS[job_id] = {"status": "running"}
        # Evict completed jobs older than 10 minutes
        now = time.time()
        stale = [k for k, v in list(_ASYNC_JOBS.items())
                 if v.get("status") != "running" and v.get("finished_at", now) < now - 600]
        for k in stale[:100]:
            _ASYNC_JOBS.pop(k, None)
    background_tasks.add_task(_async_job_run, job_id, data, user)
    return {"job_id": job_id}


@router.get("/sql/async/{job_id}")
def get_async_job(job_id: str, user: str = Depends(require_auth)):
    """Poll the status and result of an async query job."""
    with _ASYNC_JOBS_LOCK:
        job = dict(_ASYNC_JOBS.get(job_id, {}))
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    return job


@router.delete("/sql/async/{job_id}")
def cancel_async_job(job_id: str, user: str = Depends(require_auth)):
    """Remove a completed or running job from the store."""
    with _ASYNC_JOBS_LOCK:
        _ASYNC_JOBS.pop(job_id, None)
    return {"ok": True}


@router.delete("/sql/cache")
def invalidate_cache(ctx=Depends(require_min_role("Admin"))):
    """Clear the entire query result cache (admin / manual refresh)."""
    with _QUERY_CACHE_LOCK:
        _QUERY_CACHE.clear()
    return {"ok": True, "cleared": True}
