"""Catalog explorer: what KaveonDB knows about a table, read from its definition.

Browse (sources, schemas, tables) reuses the `/lab/engine/*` endpoints so the
Catalog page and the SQL Lab tree can never disagree. This router adds the one
thing the tree does not carry: the complete table definition — location, access
pattern, format, revision and typed columns — read through catalog metadata,
never by running SQL. Locations are storage paths; definitions hold credential
references only, so nothing secret can appear here.
"""
import json

from fastapi import APIRouter, Depends, HTTPException, Response

import database.metadata as db
from middleware.auth import UserContext
from middleware.permissions import require_min_role
from routers import lab
from services import engine_bridge
from services.datasets import _vis_clause

router = APIRouter(tags=["catalog"])

_ACCESS = {"Shortcut", "Optimized"}
_FORMATS = {"Parquet", "Delta", "Iceberg"}


def _text(value):
    return value if isinstance(value, str) else None


@router.get("/catalog/{source_id}/schemas/{schema}/tables/{table}")
def get_table_definition(source_id: str, schema: str, table: str, response: Response,
                         ctx: UserContext = Depends(require_min_role("Viewer"))):
    response.headers.update(lab.NO_CACHE)
    source = lab._engine_source(source_id)
    definition = engine_bridge.table_definition(source["engine_catalog"], schema, table, ctx.email, ctx.role)
    columns = []
    for column in definition["columns"]:
        if not isinstance(column, dict) or not isinstance(column.get("name"), str) or "data_type" not in column:
            raise HTTPException(502, "Engine table definition contains an invalid column")
        columns.append({
            "name": column["name"],
            "dataType": column["data_type"] if isinstance(column["data_type"], str) else str(column["data_type"]),
            "isNullable": bool(column.get("nullable", True)),
        })
    access = _text(definition.get("access"))
    fmt = _text(definition.get("format"))
    return {
        "success": True,
        "table": {
            "catalog": source["engine_catalog"],
            "schema": schema,
            "name": definition.get("name") if isinstance(definition.get("name"), str) else table,
            "location": _text(definition.get("location")),
            "access": access if access in _ACCESS else None,
            "format": fmt if fmt in _FORMATS else None,
            "revision": definition.get("revision") if isinstance(definition.get("revision"), int) else None,
            "lifecycle": _text(definition.get("lifecycle")),
            "columns": columns,
        },
    }


def _ids(value):
    """Chart ids stored on a dashboard as a JSON list; tolerate strings and ints."""
    try:
        items = json.loads(value) if isinstance(value, str) else (value or [])
    except (TypeError, ValueError):
        return set()
    return {str(item) for item in items if isinstance(item, (str, int))}


@router.get("/catalog/{source_id}/schemas/{schema}/tables/{table}/usage")
def get_table_usage(source_id: str, schema: str, table: str, response: Response,
                    ctx: UserContext = Depends(require_min_role("Viewer"))):
    """What in Kaveon reads this table: datasets, their charts, the dashboards
    those charts sit on, and whether the DLM has compiled context for it. Every
    list is filtered by the caller's visibility, exactly as the Library is."""
    response.headers.update(lab.NO_CACHE)
    catalog = lab._engine_source(source_id)["engine_catalog"]
    # Match the dataset's fact table, or a table it declares in tables_used.
    datasets = db.query(
        f"SELECT d.id, d.dataset_name, d.visibility, d.created_by FROM dbo.datasets d "
        f"WHERE d.database_name = @param0 AND d.schema_name = @param1 "
        f"AND (d.fact_table = @param2 OR d.tables_used LIKE @param3) AND {_vis_clause(4, 5)} "
        f"ORDER BY d.dataset_name",
        [catalog, schema, table, f'%"{table}"%', ctx.role, ctx.email],
    ).get("rows") or []
    dataset_ids = [row["id"] for row in datasets]
    charts, dashboards, dlm = [], [], []
    if dataset_ids:
        placeholders = ", ".join(f"@param{i}" for i in range(len(dataset_ids)))
        r, e = len(dataset_ids), len(dataset_ids) + 1
        charts = db.query(
            f"SELECT c.id, c.name, c.dataset_id FROM dbo.charts c WHERE c.dataset_id IN ({placeholders}) "
            f"AND {_vis_clause(r, e, 'c')} ORDER BY c.name",
            [*dataset_ids, ctx.role, ctx.email],
        ).get("rows") or []
        chart_ids = {str(row["id"]) for row in charts}
        if chart_ids:
            candidates = db.query(
                f"SELECT d.id, d.name, d.slug, d.charts FROM dbo.dashboards d WHERE {_vis_clause(0, 1)} ORDER BY d.name",
                [ctx.role, ctx.email],
            ).get("rows") or []
            dashboards = [
                {"id": row["id"], "name": row["name"], "slug": row.get("slug")}
                for row in candidates if _ids(row.get("charts")) & chart_ids
            ]
        dlm = db.query(
            f"SELECT dataset_id, status, built_at, stats_rollup FROM dbo.dlm_artifact WHERE dataset_id IN ({placeholders})",
            [str(i) for i in dataset_ids],
        ).get("rows") or []
    def rollup(row):
        try:
            stats = json.loads(row.get("stats_rollup") or "{}") if isinstance(row.get("stats_rollup"), str) else (row.get("stats_rollup") or {})
        except (TypeError, ValueError):
            stats = {}
        counts = stats.get("row_counts") or {}
        return {
            "datasetId": row["dataset_id"], "status": row.get("status"), "builtAt": row.get("built_at"),
            "rowCount": max((v for v in counts.values() if isinstance(v, int)), default=None) if isinstance(counts, dict) else None,
            "rowCountSource": stats.get("row_count_source"),
        }
    return {
        "success": True,
        "datasets": [{"id": row["id"], "name": row["dataset_name"], "visibility": row["visibility"]} for row in datasets],
        "charts": [{"id": row["id"], "name": row["name"], "datasetId": row["dataset_id"]} for row in charts],
        "dashboards": dashboards,
        "dlm": [rollup(row) for row in dlm],
    }
