"""Data sources router — /api/v1/data-sources."""

import logging

from fastapi import APIRouter, Request, Response, HTTPException, Depends
from middleware.auth import require_auth
from middleware.permissions import require_min_role
import database.metadata as db
import database.pool as pool
from services.credentials import encrypt, CredentialError
from services import source_mutations, source_cutover_mutations, source_secret_store, product_shadow_read, product_read_authority

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}

# connection_string is never returned to clients — it may contain credentials
_PUBLIC_FIELDS = "ds.id, ds.name, ds.type, ds.database_name, ds.region, ds.description, ds.created_by, ds.created_at, ds.updated_at, ds.is_active"

_LIST_SELECT = f"""
    SELECT {_PUBLIC_FIELDS},
           CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
    FROM data_sources ds
    LEFT JOIN favorites fav
      ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
      AND fav.object_type = 'data_source' AND fav.user_email = @param0
    ORDER BY is_favorite DESC, ds.is_active DESC, ds.created_at DESC
"""


def _add_table_counts(data_sources: list) -> list:
    results = []
    for ds in data_sources:
        count = 0
        try:
            if ds.get("database_name"):
                tables = pool.get_tables(ds["database_name"])
                count = len(tables)
        except Exception:
            pass
        results.append({**ds, "table_count": count})
    return results


def _observe_sources(rows: list[dict], user: str):
    try:
        report = product_shadow_read.observe_source_list(rows, "data", user, "Viewer")
        if report.get("enabled"):
            logging.getLogger(__name__).info("data_source_shadow_read %s", report)
    except Exception as error:
        logging.getLogger(__name__).warning("data_source_shadow_read_error type=%s", type(error).__name__)


def _cutover_sources(user: str, *, active_only: bool = False) -> list[dict]:
    documents = product_read_authority.list_documents("sources", user, "Viewer")
    rows = []
    for document in documents:
        if document.get("source_kind") != "data" or active_only and not document.get("is_active"):
            continue
        source_id = str(document.get("source_id") or "")
        if not source_id.startswith("data-"):
            raise RuntimeError("KaveonDB data-source identity is invalid")
        rows.append({
            "id": source_id[5:], "name": document.get("name"),
            "type": document.get("source_type"), "database_name": document.get("database_name"),
            "region": document.get("region"), "description": document.get("description"),
            "created_by": None, "created_at": None, "updated_at": None,
            "is_active": bool(document.get("is_active")), "is_favorite": bool(document.get("favorite")),
        })
    return sorted(rows, key=lambda row: (bool(row["is_favorite"]), bool(row["is_active"]), str(row["id"])), reverse=True)


@router.get("/data-sources")
def list_data_sources(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    if product_read_authority.enabled("sources"):
        return {"success": True, "dataSources": [{**row, "table_count": 0} for row in _cutover_sources(user)]}
    result = db.query(_LIST_SELECT, [user])
    _observe_sources(result["rows"], user)
    return {"success": True, "dataSources": _add_table_counts(result["rows"])}


@router.get("/data-sources/active")
def list_active_data_sources(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    if product_read_authority.enabled("sources"):
        return {"success": True, "dataSources": [{**row, "table_count": 0} for row in _cutover_sources(user, active_only=True)]}
    result = db.query(f"""
        SELECT {_PUBLIC_FIELDS},
               CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
        FROM data_sources ds
        LEFT JOIN favorites fav
          ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
          AND fav.object_type = 'data_source' AND fav.user_email = @param0
        WHERE ds.is_active = 1
        ORDER BY is_favorite DESC, ds.created_at DESC
    """, [user])
    _observe_sources(result["rows"], user)
    return {"success": True, "dataSources": _add_table_counts(result["rows"])}


@router.get("/data-sources/list")
def list_data_sources_metadata_only(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
    if product_read_authority.enabled("sources"):
        return {"success": True, "dataSources": _cutover_sources(user)}
    result = db.query(f"""
        SELECT {_PUBLIC_FIELDS},
               CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
        FROM data_sources ds
        LEFT JOIN favorites fav
          ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
          AND fav.object_type = 'data_source' AND fav.user_email = @param0
        ORDER BY is_favorite DESC, ds.is_active DESC, ds.created_at DESC
    """, [user])
    _observe_sources(result["rows"], user)
    return {"success": True, "dataSources": result["rows"]}


@router.get("/data-sources/favorite/current")
def get_favorite_data_source(user: str = Depends(require_auth)):
    result = db.query(f"""
        SELECT {_PUBLIC_FIELDS}
        FROM favorites fav
        INNER JOIN data_sources ds ON fav.object_id = CAST(ds.id AS NVARCHAR(255))
        WHERE fav.user_email = @param0 AND fav.object_type = 'data_source'
    """, [user])
    row = result["rows"][0] if result["rows"] else None
    if row is not None:
        try:
            report = product_shadow_read.observe_source(row, "data", user, "Viewer")
            if report.get("enabled"):
                logging.getLogger(__name__).info("data_source_shadow_read %s", report)
        except Exception as error:
            logging.getLogger(__name__).warning("data_source_shadow_read_error type=%s", type(error).__name__)
    return {"success": True, "dataSource": row}


@router.get("/data-sources/{ds_id}/table-count")
def get_table_count(ds_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return {"success": True, "tableCount": None, "message": "Table counting disabled for performance"}


@router.get("/data-sources/{ds_id}")
def get_data_source(ds_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    if product_read_authority.enabled("sources"):
        document = product_read_authority.read_document("sources", f"data-{int(ds_id)}", user, "Viewer")
        rows = _cutover_sources(user)
        row = next((row for row in rows if str(row["id"]) == str(ds_id)), None) if document else None
        if row is None: raise HTTPException(status_code=404, detail="Data source not found")
        return {"success": True, "dataSource": row}
    result = db.query(
        f"SELECT {_PUBLIC_FIELDS} FROM data_sources ds WHERE ds.id = @param0",
        [int(ds_id)]
    )
    if not result["rows"]:
        raise HTTPException(status_code=404, detail="Data source not found")
    try:
        report = product_shadow_read.observe_source(result["rows"][0], "data", user, "Viewer")
        if report.get("enabled"):
            logging.getLogger(__name__).info("data_source_shadow_read %s", report)
    except Exception as error:
        logging.getLogger(__name__).warning("data_source_shadow_read_error type=%s", type(error).__name__)
    return {"success": True, "dataSource": result["rows"][0]}


@router.post("/data-sources", status_code=201)
def create_data_source(data: dict, ctx=Depends(require_min_role("Admin"))):
    user = ctx.email
    name = data.get("name")
    ds_type = data.get("type")
    connection_string = data.get("connection_string")
    region = data.get("region")
    if not name or not ds_type or not connection_string or not region:
        raise HTTPException(status_code=400, detail="Missing required fields: name, type, connection_string, region")
    if ds_type != "StarRocks" and not data.get("database_name"):
        raise HTTPException(status_code=400, detail=f"Database name is required for {ds_type} data sources")
    if region not in ("WW", "EU"):
        raise HTTPException(status_code=400, detail='Region must be either "WW" or "EU"')

    if source_cutover_mutations.enabled():
        source_id = source_cutover_mutations.new_data_id()
        store = source_secret_store.SourceSecretStore()
        try:
            secret_ref = store.set("data", source_id, connection_string)
            row = {"id": source_id, "name": name, "type": ds_type, "database_name": data.get("database_name"),
                   "region": region, "description": data.get("description"), "created_by": user,
                   "modified_by": user, "is_active": True, "secret_ref": secret_ref}
            source_cutover_mutations.create("data_sources", row, user)
        except source_secret_store.SourceSecretError as error:
            raise HTTPException(503, str(error)) from None
        except Exception:
            try: store.delete(secret_ref)
            except Exception: pass
            raise
        return {"success":True,"dataSource":{
            "id":source_id,"name":name,"type":ds_type,"database_name":data.get("database_name"),"region":region,
            "description":data.get("description"),"created_by":user,"is_active":True},"message":"Data source created successfully"}

    try:
        connection_string = encrypt(connection_string)
    except CredentialError:
        raise HTTPException(503, "Credential encryption is unavailable") from None

    try:
        # Use OUTPUT INSERTED but only return public fields (not connection_string)
        with db.transaction() as transaction:
            inserted = transaction.query_one("""
            INSERT INTO data_sources (name, type, connection_string, database_name,
                                      region, description, created_by, is_active)
            VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7)
            RETURNING id, name, type, database_name, region, description, created_by, created_at, updated_at, is_active
        """, [name, ds_type, connection_string, data.get("database_name"), region,
              data.get("description"), user, True])
            if not inserted: raise RuntimeError("data source insert returned no row")
            source_mutations.enqueue(transaction,"data_sources","create",inserted,user)
        return {"success": True, "dataSource": inserted, "message": "Data source created successfully"}
    except Exception as e:
        msg = str(e).lower()
        if "unique" in msg or "duplicate" in msg:
            raise HTTPException(status_code=409, detail="A data source with this name already exists")
        raise HTTPException(status_code=500, detail="Failed to create data source")


@router.patch("/data-sources/{ds_id}")
def update_data_source(ds_id: str, data: dict, ctx=Depends(require_min_role("Admin"))):
    user = ctx.email  # noqa: F841
    region = data.get("region")
    if region and region not in ("WW", "EU"):
        raise HTTPException(status_code=400, detail='Region must be either "WW" or "EU"')
    if source_cutover_mutations.enabled():
        current = product_read_authority.read_document("sources",f"data-{int(ds_id)}",user,"Admin")
        if not current: raise HTTPException(404,"Data source not found")
        changes={}
        for source,target in (("name","name"),("type","source_type"),("database_name","database_name"),("region","region"),("description","description"),("is_active","is_active")):
            if source in data: changes[target]=data[source]
        if "is_active" in changes: changes["lifecycle"]="active" if changes["is_active"] else "suspended"
        if "connection_string" in data:
            if not isinstance(data["connection_string"],str) or not data["connection_string"]:
                raise HTTPException(400,"connection_string must be nonempty")
            try: changes["secret_ref"]=source_secret_store.SourceSecretStore().set("data",str(ds_id),data["connection_string"])
            except source_secret_store.SourceSecretError as error: raise HTTPException(503,str(error)) from None
        if not changes: raise HTTPException(400,"No fields to update")
        updated=source_cutover_mutations.update(f"data-{int(ds_id)}",changes,user)
        return {"success":True,"dataSource":{"id":str(ds_id),"name":updated.get("name"),"type":updated.get("source_type"),
            "database_name":updated.get("database_name"),"region":updated.get("region"),"description":updated.get("description"),
            "created_by":updated.get("created_by"),"is_active":updated.get("is_active")},"message":"Data source updated successfully"}

    if "connection_string" in data:
        if not isinstance(data["connection_string"], str) or not data["connection_string"]:
            raise HTTPException(400, "connection_string must be nonempty")
        try:
            data = {**data, "connection_string": encrypt(data["connection_string"])}
        except CredentialError:
            raise HTTPException(503, "Credential encryption is unavailable") from None

    updates, params, i = [], [], 0
    for field, col in [("name", "name"), ("type", "type"),
                       ("connection_string", "connection_string"),
                       ("database_name", "database_name"), ("region", "region"),
                       ("description", "description")]:
        if field in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field] or None); i += 1
    if "is_active" in data:
        updates.append(f"is_active = @param{i}"); params.append(bool(data["is_active"])); i += 1

    if not updates:
        raise HTTPException(status_code=400, detail="No fields to update")

    updates.append("updated_at = GETDATE()")
    params.append(int(ds_id))

    try:
        with db.transaction() as transaction:
            existing = transaction.query_one("SELECT id FROM data_sources WHERE id=@param0 FOR UPDATE",[int(ds_id)])
            if not existing: raise HTTPException(status_code=404, detail="Data source not found")
            updated = transaction.query_one(
            f"UPDATE data_sources SET {', '.join(updates)} WHERE id = @param{i} RETURNING id,name,type,database_name,region,description,created_by,is_active",
            params + []
            )
            if updated is None: raise RuntimeError("data source update lost its row lock")
            source_mutations.enqueue(transaction,"data_sources","update",updated,user)
        return {"success": True, "dataSource": updated, "message": "Data source updated successfully"}
    except HTTPException:
        raise
    except Exception as e:
        msg = str(e).lower()
        if "unique" in msg or "duplicate" in msg:
            raise HTTPException(status_code=409, detail="A data source with this name already exists")
        raise HTTPException(status_code=500, detail="Failed to update data source")


@router.delete("/data-sources/{ds_id}")
def delete_data_source(ds_id: str, ctx=Depends(require_min_role("Admin"))):
    if source_cutover_mutations.enabled():
        record_id=f"data-{int(ds_id)}"
        current=product_read_authority.read_document("sources",record_id,ctx.email,"Admin")
        if not current: raise HTTPException(404,"Data source not found")
        favorites=product_read_authority.list_documents("favorites",ctx.email,"Admin") if product_read_authority.enabled("favorites") else []
        if any(item.get("object_type")=="source" and item.get("object_id")==record_id for item in favorites):
            raise HTTPException(409,"Remove data-source favorites before deletion")
        source_cutover_mutations.delete(record_id,ctx.email)
        try: source_secret_store.SourceSecretStore().delete(str(current.get("secret_ref")))
        except source_secret_store.SourceSecretError: pass
        return {"success":True,"message":"Data source deleted successfully"}
    with db.transaction() as transaction:
        row=transaction.query_one("SELECT id,name,type,database_name,region,description,created_by,is_active FROM data_sources WHERE id=@param0 FOR UPDATE",[int(ds_id)])
        if not row: raise HTTPException(status_code=404, detail="Data source not found")
        dependent=transaction.query_one("SELECT COUNT(*) AS count FROM favorites WHERE object_type='data_source' AND object_id=@param0",[str(ds_id)]) or {}
        if dependent.get("count"): raise HTTPException(409,"Remove data-source favorites before deletion")
        count=transaction.execute("DELETE FROM data_sources WHERE id=@param0",[int(ds_id)])
        if count != 1: raise RuntimeError("data source delete lost its row lock")
        source_mutations.enqueue(transaction,"data_sources","delete",row,ctx.email)
    return {"success": True, "message": "Data source deleted successfully"}


@router.post("/data-sources/{ds_id}/test")
def test_data_source(ds_id: str, user: str = Depends(require_auth)):
    row = db.query_one("SELECT database_name, type FROM data_sources WHERE id = @param0", [int(ds_id)])
    if not row:
        raise HTTPException(status_code=404, detail="Data source not found")
    return {"success": True, "message": "Connection test not yet implemented",
            "database": row.get("database_name"), "type": row.get("type")}


@router.post("/data-sources/{ds_id}/favorite")
def set_ds_favorite(ds_id: str, user: str = Depends(require_auth)):
    ds = db.query_one("SELECT id, name FROM data_sources WHERE id = @param0", [int(ds_id)])
    if not ds:
        raise HTTPException(status_code=404, detail="Data source not found")
    db.execute("DELETE FROM favorites WHERE user_email = @param0 AND object_type = 'data_source'", [user])
    db.execute(
        "INSERT INTO favorites (user_email, object_id, object_type, object_name, created_at) "
        "VALUES (@param0, @param1, 'data_source', @param2, GETDATE())",
        [user, str(ds_id), ds.get("name", "Unknown")]
    )
    return {"success": True, "message": "Data source set as favorite"}


@router.delete("/data-sources/{ds_id}/favorite")
def remove_ds_favorite(ds_id: str, user: str = Depends(require_auth)):
    db.execute(
        "DELETE FROM favorites WHERE user_email = @param0 AND object_id = @param1 AND object_type = 'data_source'",
        [user, str(ds_id)]
    )
    return {"success": True, "message": "Favorite removed"}
