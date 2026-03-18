"""SQL router — /api/v1/sql."""

import json
import time
from fastapi import APIRouter, Request, Response, HTTPException, Query, Depends
from middleware.auth import require_auth
import services.datasets as datasets_svc
import services.query_history as history_svc
from services.query_generator import build_chart_preview_query, build_distinct_filter_values_query
from services.sql_table_extractor import extract_tables_from_sql
import database.pool as pool

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}

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
def generate_sql(data: dict, user: str = Depends(require_auth)):
    dataset_id = data.get("dataset_id")
    chart_type = data.get("chart_type")
    config = data.get("config")

    if not dataset_id or not chart_type or not config:
        raise HTTPException(status_code=400, detail="dataset_id, chart_type, and config are required")

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

    datasource = (
        f"{dataset['schema_name']}.{dataset['table_name']}"
        if dataset.get("schema_name")
        else dataset.get("table_name") or ""
    )

    params = {
        "datasource": datasource,
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
        **({} if not dashboard_id else {"dashboardId": int(dashboard_id)}),
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
def execute_sql(data: dict, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    user_id = user

    sql_text = data.get("sql_text") or ""
    database = data.get("database") or ""
    source = data.get("source")
    tables_used = data.get("tables_used")
    chart_id = data.get("chart_id")
    dashboard_id = data.get("dashboard_id")
    chart_type = data.get("chart_type")
    dataset_id = data.get("dataset_id")
    row_limit = data.get("row_limit")

    if not sql_text:
        raise HTTPException(status_code=400, detail="sql_text is required")
    if not database:
        raise HTTPException(status_code=400, detail="database parameter is required")

    trigger_source = canonical_source(source)
    resolved_tables = tables_used or json.dumps(extract_tables_from_sql(sql_text))
    run_context = json.dumps({
        "source": trigger_source, "database": database,
        **({} if chart_id is None else {"chartId": int(chart_id)}),
        **({} if dashboard_id is None else {"dashboardId": int(dashboard_id)}),
        **({} if chart_type is None else {"chartType": chart_type}),
        **({} if dataset_id is None else {"datasetId": int(dataset_id)}),
    })

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
        except Exception:
            pass
        raise HTTPException(status_code=500, detail="Query execution failed")

    duration_ms = int(time.time() * 1000) - start_time
    try:
        history_svc.create_history({
            "sql_text": sql_text, "duration_ms": duration_ms,
            "row_count": result.get("row_count") or 0, "status": "success",
            "trigger_source": trigger_source, "tables_used": resolved_tables,
            "run_context": run_context,
            "dataset_id": int(dataset_id) if dataset_id is not None else None,
            "started_at": start_time,
        }, user_id)
    except Exception:
        pass

    row_count = result.get("row_count") or 0
    return {
        "columns": result.get("columns") or [],
        "rows": result.get("rows") or [],
        "message": f"Returned {row_count} rows" if row_count else None,
        "duration_ms": duration_ms,
    }
