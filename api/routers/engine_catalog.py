"""Engine catalog management — /api/v1/engine/catalog.

Adding catalogs, schemas and tables the way a Trino user expects: from the
platform's API and from Studio, by name, with the Engine's durable ids and
optimistic revisions handled here. Every mutation goes through the server-side
bridge with the Engine's catalog-admin credential; the browser never holds it.

Roles. The Engine itself knows one catalog-admin credential, so the platform's
role model is the boundary: Admin registers catalogs (a storage location and a
credential reference are infrastructure), Editor registers schemas and tables
inside them, everyone with a role can read definitions. Catalog access grants
narrow that further: the Engine answers definition reads for the verified
principal only with the catalogs they were granted, and reports the level on
each (`access`); a schema or table change inside a catalog needs `manage`.

A table is registered as Draft, activated, and read once (`SELECT COUNT(*)`
with the result cache bypassed) before the call returns. The Engine publishes
only Active definitions into its query snapshot, so the read has to follow the
activation; a table the Engine cannot read is deleted again and the storage
error is returned verbatim, because the person registering it needs the path
or schema mismatch the Engine names.
"""
from fastapi import APIRouter, Depends, Header, HTTPException, Response

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from models.engine_catalog import CatalogCreate, SchemaCreate, TableCreate, TableReplace
from routers import lab
from services import engine_bridge

router = APIRouter(tags=["engine-catalog"])

_ACTIVE = "Active"


def _revision(if_match: str | None) -> int:
    """The caller's current revision, as the Engine's If-Match. Required for every replace or delete."""
    if if_match is None:
        raise HTTPException(428, {"code": "revision_required",
                                  "message": "Send the definition's current revision in If-Match."})
    value = if_match.strip().strip('"')
    if not value.isdigit() or int(value) < 1:
        raise HTTPException(400, {"code": "invalid_revision", "message": "If-Match must be a positive revision number."})
    return int(value)


def _require_manage(catalog: dict) -> None:
    """A change inside `catalog` needs the caller's `manage` level, as the
    Engine reported it on the definition. A definition the Engine did not
    annotate is refused: access is never assumed."""
    if catalog.get("access") != "manage":
        raise HTTPException(403, {"code": "catalog_access",
                                  "message": f"Changing {catalog.get('name')} requires manage access on the catalog."})


def _schema_parents(schema_id: str, ctx: UserContext) -> tuple[dict, dict]:
    schema = engine_bridge.schema_definition(schema_id, ctx.email, ctx.role)
    if not isinstance(schema, dict) or not isinstance(schema.get("catalog_id"), str):
        raise HTTPException(404, {"code": "schema_not_found", "message": "Schema definition not found."})
    catalog = engine_bridge.catalog_definition(schema["catalog_id"], ctx.email, ctx.role)
    if not isinstance(catalog, dict) or not isinstance(catalog.get("name"), str):
        raise HTTPException(404, {"code": "catalog_not_found", "message": "Catalog definition not found."})
    return catalog, schema


# ── Catalogs ─────────────────────────────────────────────────────────────────

@router.get("/engine/catalog/definitions")
def list_catalog_definitions(response: Response, ctx: UserContext = Depends(require_min_role("Viewer"))):
    response.headers.update(lab.NO_CACHE)
    return {"success": True, "definitions": engine_bridge.catalog_definitions(ctx.email, ctx.role)}


@router.post("/engine/catalog/definitions", status_code=201)
def create_catalog_definition(body: CatalogCreate, ctx: UserContext = Depends(require_min_role("Admin"))):
    """Register a catalog: a platform source record, activated and synchronized
    to the Engine in one call. The platform registry stays the one registration
    path, so the catalog appears in Studio and SQL Lab as soon as it exists."""
    from routers import catalog_sources
    storage = body.storage.model_dump()
    storage_type = storage.pop("type")
    record = {
        "name": body.name, "engine_catalog": body.name, "storage_type": storage_type,
        "storage_config": storage, "data_format": body.format, "adapter_type": "native", "adapter_config": {},
        "credential_kind": body.credential.kind if body.credential else None,
        "credential_ref": body.credential.reference if body.credential else None,
        "description": body.description,
    }
    source = catalog_sources.create_catalog_source(record, ctx)["catalogSource"]
    source_id = str(source["id"])
    source = catalog_sources.transition_lifecycle(source_id, {"lifecycle": "active"}, ctx)["catalogSource"]
    synced = catalog_sources.sync_engine_catalog(source_id, {}, ctx)
    return {"success": True, "catalog": synced["catalog"], "source": {"id": source_id, "name": source["name"]}}


# ── Schemas ──────────────────────────────────────────────────────────────────

@router.get("/engine/catalog/definitions/{catalog_id}/schemas")
def list_schema_definitions(catalog_id: str, response: Response,
                            ctx: UserContext = Depends(require_min_role("Viewer"))):
    response.headers.update(lab.NO_CACHE)
    if engine_bridge.catalog_definition(catalog_id, ctx.email, ctx.role) is None:
        raise HTTPException(404, {"code": "catalog_not_found", "message": "Catalog definition not found."})
    return {"success": True, "schemas": engine_bridge.schema_definitions(catalog_id, ctx.email, ctx.role)}


@router.post("/engine/catalog/definitions/{catalog_id}/schemas", status_code=201)
def create_schema_definition(catalog_id: str, body: SchemaCreate,
                             ctx: UserContext = Depends(require_min_role("Editor"))):
    catalog = engine_bridge.catalog_definition(catalog_id, ctx.email, ctx.role)
    if not isinstance(catalog, dict):
        raise HTTPException(404, {"code": "catalog_not_found", "message": "Catalog definition not found."})
    _require_manage(catalog)
    if catalog.get("lifecycle") != _ACTIVE:
        raise HTTPException(409, {"code": "catalog_inactive",
                                  "message": f"Catalog {catalog.get('name')} is {str(catalog.get('lifecycle')).lower()}; activate it before adding schemas."})
    schema_id = body.id or f"{catalog_id}-{body.name}"
    schema = engine_bridge.create_schema(catalog_id, schema_id, body.name, ctx.email)
    return {"success": True, "schema": schema}


@router.delete("/engine/catalog/schemas/{schema_id}", status_code=204)
def delete_schema_definition(schema_id: str, if_match: str | None = Header(default=None),
                             ctx: UserContext = Depends(require_min_role("Editor"))):
    """Remove an empty schema. A schema that still holds tables is refused by the Engine."""
    revision = _revision(if_match)
    catalog, _ = _schema_parents(schema_id, ctx)
    _require_manage(catalog)
    engine_bridge.delete_schema(schema_id, revision, ctx.email)
    return Response(status_code=204)


# ── Tables ───────────────────────────────────────────────────────────────────

@router.get("/engine/catalog/schemas/{schema_id}/tables")
def list_table_definitions(schema_id: str, response: Response,
                           ctx: UserContext = Depends(require_min_role("Viewer"))):
    response.headers.update(lab.NO_CACHE)
    if engine_bridge.schema_definition(schema_id, ctx.email, ctx.role) is None:
        raise HTTPException(404, {"code": "schema_not_found", "message": "Schema definition not found."})
    return {"success": True, "tables": engine_bridge.table_definitions(schema_id, ctx.email, ctx.role)}


@router.get("/engine/catalog/tables/{table_id}")
def get_table_definition(table_id: str, response: Response,
                         ctx: UserContext = Depends(require_min_role("Viewer"))):
    response.headers.update(lab.NO_CACHE)
    table = engine_bridge.table_definition_by_id(table_id, ctx.email, ctx.role)
    if not isinstance(table, dict):
        raise HTTPException(404, {"code": "table_not_found", "message": "Table definition not found."})
    return {"success": True, "table": table}


@router.post("/engine/catalog/tables", status_code=201)
def create_table_definition(body: TableCreate, ctx: UserContext = Depends(require_min_role("Editor"))):
    catalog, schema = _schema_parents(body.schema_id, ctx)
    _require_manage(catalog)
    if body.verify and (catalog.get("lifecycle") != _ACTIVE or schema.get("lifecycle") != _ACTIVE):
        raise HTTPException(409, {"code": "parent_inactive",
                                  "message": "The catalog and schema must be active for the table to be verified."})
    if not body.columns:
        # No column list: the Engine's CREATE TABLE infers the columns from the
        # table's metadata and registers only a readable location.
        created = engine_bridge.create_table_inferred(
            catalog["name"], schema["name"], body.name, body.location, body.format, ctx.email, ctx.role)
        if not created["ok"]:
            raise HTTPException(422, {
                "code": "table_unreadable", "message": created["message"], "engineCode": created["code"],
                "table": {"id": None, "name": body.name, "location": body.location, "format": body.format},
                "removed": True,
            })
        table = next((t for t in engine_bridge.table_definitions(body.schema_id, ctx.email, ctx.role) or []
                      if isinstance(t, dict) and t.get("name") == body.name), None)
        if not body.verify:
            return {"success": True, "table": table, "probe": None}
        probe = engine_bridge.probe_table(catalog["name"], schema["name"], body.name, ctx.email, ctx.role)
        return {"success": True, "table": table,
                "probe": ({"rowCount": probe["row_count"], "elapsedMs": probe["elapsed_ms"],
                           "queryId": probe["query_id"]} if probe["ok"] else None)}
    definition = {
        "id": body.id or f"{body.schema_id}-{body.name}", "schema_id": body.schema_id, "name": body.name,
        "location": body.location, "access": body.access, "format": body.format,
        "columns": [column.engine() for column in body.columns],
    }
    table = engine_bridge.create_table(definition, ctx.email)
    if not body.verify:
        return {"success": True, "table": table, "probe": None}
    probe = engine_bridge.probe_table(catalog["name"], schema["name"], body.name, ctx.email, ctx.role)
    if probe["ok"]:
        return {"success": True, "table": table,
                "probe": {"rowCount": probe["row_count"], "elapsedMs": probe["elapsed_ms"], "queryId": probe["query_id"]}}
    # The Engine cannot read the location: take the definition back out so a
    # broken table is never left in the catalog, then say exactly what failed.
    removed = True
    try:
        engine_bridge.delete_table(table["id"], table["revision"], ctx.email)
    except HTTPException:
        removed = False
    raise HTTPException(422, {
        "code": "table_unreadable", "message": probe["message"], "engineCode": probe["code"],
        "table": {"id": table["id"], "name": body.name, "location": body.location, "format": body.format},
        "removed": removed,
    })


@router.put("/engine/catalog/tables/{table_id}")
def replace_table_definition(table_id: str, body: TableReplace, if_match: str | None = Header(default=None),
                             ctx: UserContext = Depends(require_min_role("Editor"))):
    revision = _revision(if_match)
    current = engine_bridge.table_definition_by_id(table_id, ctx.email, ctx.role)
    if not isinstance(current, dict):
        raise HTTPException(404, {"code": "table_not_found", "message": "Table definition not found."})
    catalog, _ = _schema_parents(current["schema_id"], ctx)
    _require_manage(catalog)
    definition = {
        "id": table_id, "schema_id": current["schema_id"], "name": body.name,
        "location": body.location, "access": body.access, "format": body.format,
        "columns": [column.engine() for column in body.columns],
        "lifecycle": body.lifecycle or current.get("lifecycle", _ACTIVE),
    }
    return {"success": True, "table": engine_bridge.replace_table(definition, revision, ctx.email)}


@router.delete("/engine/catalog/tables/{table_id}", status_code=204)
def delete_table_definition(table_id: str, if_match: str | None = Header(default=None),
                            ctx: UserContext = Depends(require_min_role("Editor"))):
    """Remove a table definition. The data at its location is not touched."""
    revision = _revision(if_match)
    current = engine_bridge.table_definition_by_id(table_id, ctx.email, ctx.role)
    if not isinstance(current, dict):
        raise HTTPException(404, {"code": "table_not_found", "message": "Table definition not found."})
    catalog, _ = _schema_parents(current["schema_id"], ctx)
    _require_manage(catalog)
    engine_bridge.delete_table(table_id, revision, ctx.email)
    return Response(status_code=204)
