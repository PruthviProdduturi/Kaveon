"""Data sources router — /api/v1/data-sources."""

from fastapi import APIRouter, Request, Response, HTTPException, Depends
from middleware.auth import require_auth
from middleware.permissions import require_min_role
import database.metadata as db
import database.pool as pool

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


@router.get("/data-sources")
def list_data_sources(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    result = db.query(_LIST_SELECT, [user])
    return {"success": True, "dataSources": _add_table_counts(result["rows"])}


@router.get("/data-sources/active")
def list_active_data_sources(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
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
    return {"success": True, "dataSources": _add_table_counts(result["rows"])}


@router.get("/data-sources/list")
def list_data_sources_metadata_only(request: Request, response: Response, user: str = Depends(require_auth)):
    response.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
    result = db.query(f"""
        SELECT {_PUBLIC_FIELDS},
               CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
        FROM data_sources ds
        LEFT JOIN favorites fav
          ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
          AND fav.object_type = 'data_source' AND fav.user_email = @param0
        ORDER BY is_favorite DESC, ds.is_active DESC, ds.created_at DESC
    """, [user])
    return {"success": True, "dataSources": result["rows"]}


@router.get("/data-sources/favorite/current")
def get_favorite_data_source(user: str = Depends(require_auth)):
    result = db.query(f"""
        SELECT {_PUBLIC_FIELDS}
        FROM favorites fav
        INNER JOIN data_sources ds ON fav.object_id = CAST(ds.id AS NVARCHAR(255))
        WHERE fav.user_email = @param0 AND fav.object_type = 'data_source'
    """, [user])
    return {"success": True, "dataSource": result["rows"][0] if result["rows"] else None}


@router.get("/data-sources/{ds_id}/table-count")
def get_table_count(ds_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return {"success": True, "tableCount": None, "message": "Table counting disabled for performance"}


@router.get("/data-sources/{ds_id}")
def get_data_source(ds_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    result = db.query(
        f"SELECT {_PUBLIC_FIELDS} FROM data_sources ds WHERE ds.id = @param0",
        [int(ds_id)]
    )
    if not result["rows"]:
        raise HTTPException(status_code=404, detail="Data source not found")
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

    try:
        # Use OUTPUT INSERTED but only return public fields (not connection_string)
        db.execute("""
            INSERT INTO data_sources (name, type, connection_string, database_name,
                                      region, description, created_by, is_active)
            VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, 1)
        """, [name, ds_type, connection_string, data.get("database_name"), region,
              data.get("description"), user])
        inserted = db.query_one(
            f"SELECT TOP 1 {_PUBLIC_FIELDS} FROM data_sources ds WHERE ds.name = @param0 AND ds.created_by = @param1 ORDER BY ds.id DESC",
            [name, user]
        )
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

    updates, params, i = [], [], 0
    for field, col in [("name", "name"), ("type", "type"),
                       ("connection_string", "connection_string"),
                       ("database_name", "database_name"), ("region", "region"),
                       ("description", "description")]:
        if field in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field] or None); i += 1
    if "is_active" in data:
        updates.append(f"is_active = @param{i}"); params.append(1 if data["is_active"] else 0); i += 1

    if not updates:
        raise HTTPException(status_code=400, detail="No fields to update")

    updates.append("updated_at = GETDATE()")
    params.append(int(ds_id))

    try:
        count = db.execute(
            f"UPDATE data_sources SET {', '.join(updates)} WHERE id = @param{i}",
            params
        )
        if not count:
            raise HTTPException(status_code=404, detail="Data source not found")
        updated = db.query_one(f"SELECT {_PUBLIC_FIELDS} FROM data_sources ds WHERE ds.id = @param0", [int(ds_id)])
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
    count = db.execute("DELETE FROM data_sources WHERE id = @param0", [int(ds_id)])
    if not count:
        raise HTTPException(status_code=404, detail="Data source not found")
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
