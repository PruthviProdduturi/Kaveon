"""Catalog explorer: what KaveonDB knows about a table, read from its definition.

Browse (sources, schemas, tables) reuses the `/lab/engine/*` endpoints so the
Catalog page and the SQL Lab tree can never disagree. This router adds the one
thing the tree does not carry: the complete table definition — location, access
pattern, format, revision and typed columns — read through catalog metadata,
never by running SQL. Locations are storage paths; definitions hold credential
references only, so nothing secret can appear here.
"""
from fastapi import APIRouter, Depends, HTTPException, Response

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from routers import lab
from services import engine_bridge

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
