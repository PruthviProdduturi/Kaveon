"""Catalog sources router — /api/v1/catalog-sources.

Control-plane CRUD for Engine catalog definitions. Stores storage type,
credential references (Key Vault URIs — never raw secrets), adapter
configuration, and lifecycle state. Engine owns runtime resolution;
this API owns the metadata and lifecycle transitions.
"""

import json
from fastapi import APIRouter, HTTPException, Depends
from middleware.auth import UserContext
from middleware.permissions import require_min_role
import database.metadata as db

router = APIRouter()

_FIELDS = """
    id, name, engine_catalog, storage_type, storage_config, data_format,
    credential_kind, credential_ref, adapter_type, adapter_config,
    lifecycle, description, created_by, modified_by, created_at, modified_at
"""

_VALID_STORAGE = {"local", "adls_gen2", "s3"}
_VALID_FORMAT = {"parquet", "delta", "iceberg"}
_VALID_CREDENTIAL = {"managed_identity", "workload_identity", "environment", "secret_store"}
_VALID_ADAPTER = {"native", "hive_metastore", "aws_glue", "unity_catalog", "iceberg_rest"}
_VALID_LIFECYCLE = {"draft", "active", "suspended", "deleting", "deleted"}

_TRANSITIONS = {
    "draft": {"active", "deleted"},
    "active": {"suspended", "deleting"},
    "suspended": {"active", "deleting"},
    "deleting": {"deleted"},
    "deleted": set(),
}


def _audit(action: str, obj_id: str, obj_name: str, user: str, details: str = None):
    db.execute(
        "INSERT INTO activity (action, object_type, object_id, object_name, user_email, details) "
        "VALUES (@param0, 'catalog_source', @param1, @param2, @param3, @param4)",
        [action, obj_id, obj_name, user, details],
    )


def _validate_json(value: str, field_name: str) -> dict:
    try:
        parsed = json.loads(value)
        if not isinstance(parsed, dict):
            raise ValueError
        return parsed
    except (json.JSONDecodeError, ValueError, TypeError):
        raise HTTPException(400, f"{field_name} must be valid JSON object")


def _validate_storage_config(storage_type: str, config: dict):
    if storage_type == "local":
        if "base_path" not in config:
            raise HTTPException(400, "local storage requires base_path in storage_config")
    elif storage_type == "adls_gen2":
        for k in ("account", "container", "root_path"):
            if k not in config:
                raise HTTPException(400, f"adls_gen2 storage requires {k} in storage_config")
    elif storage_type == "s3":
        for k in ("bucket", "region", "prefix"):
            if k not in config:
                raise HTTPException(400, f"s3 storage requires {k} in storage_config")


@router.get("/catalog-sources")
def list_catalog_sources(ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = db.query(
        f"SELECT {_FIELDS} FROM catalog_sources WHERE lifecycle != 'deleted' ORDER BY created_at DESC"
    )
    return {"success": True, "catalogSources": result["rows"]}


@router.get("/catalog-sources/{cs_id}")
def get_catalog_source(cs_id: str, ctx: UserContext = Depends(require_min_role("Viewer"))):
    row = db.query_one(f"SELECT {_FIELDS} FROM catalog_sources WHERE id = @param0", [cs_id])
    if not row:
        raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})
    return {"success": True, "catalogSource": row}


@router.post("/catalog-sources", status_code=201)
def create_catalog_source(data: dict, ctx: UserContext = Depends(require_min_role("Admin"))):
    name = (data.get("name") or "").strip()
    engine_catalog = (data.get("engine_catalog") or "").strip()
    storage_type = (data.get("storage_type") or "").strip()
    data_format = (data.get("data_format") or "parquet").strip()
    adapter_type = (data.get("adapter_type") or "native").strip()

    if not name:
        raise HTTPException(400, "name is required")
    if not engine_catalog:
        raise HTTPException(400, "engine_catalog is required")
    if storage_type not in _VALID_STORAGE:
        raise HTTPException(400, f"storage_type must be one of: {', '.join(sorted(_VALID_STORAGE))}")
    if data_format not in _VALID_FORMAT:
        raise HTTPException(400, f"data_format must be one of: {', '.join(sorted(_VALID_FORMAT))}")
    if adapter_type not in _VALID_ADAPTER:
        raise HTTPException(400, f"adapter_type must be one of: {', '.join(sorted(_VALID_ADAPTER))}")

    storage_config_raw = data.get("storage_config") or "{}"
    if isinstance(storage_config_raw, dict):
        storage_config_raw = json.dumps(storage_config_raw)
    config = _validate_json(storage_config_raw, "storage_config")
    _validate_storage_config(storage_type, config)

    credential_kind = (data.get("credential_kind") or "").strip() or None
    credential_ref = (data.get("credential_ref") or "").strip() or None
    if credential_kind and credential_kind not in _VALID_CREDENTIAL:
        raise HTTPException(400, f"credential_kind must be one of: {', '.join(sorted(_VALID_CREDENTIAL))}")
    if credential_kind == "secret_store" and not credential_ref:
        raise HTTPException(400, "secret_store credential requires a Key Vault URI in credential_ref")

    adapter_config_raw = data.get("adapter_config") or "{}"
    if isinstance(adapter_config_raw, dict):
        adapter_config_raw = json.dumps(adapter_config_raw)
    _validate_json(adapter_config_raw, "adapter_config")

    description = (data.get("description") or "").strip() or None

    try:
        db.execute(
            "INSERT INTO catalog_sources "
            "(name, engine_catalog, storage_type, storage_config, data_format, "
            " credential_kind, credential_ref, adapter_type, adapter_config, "
            " description, created_by) "
            "VALUES (@param0, @param1, @param2, @param3, @param4, "
            "        @param5, @param6, @param7, @param8, @param9, @param10)",
            [name, engine_catalog, storage_type, storage_config_raw, data_format,
             credential_kind, credential_ref, adapter_type, adapter_config_raw,
             description, ctx.email],
        )
    except Exception as e:
        msg = str(e).lower()
        if "unique" in msg or "duplicate" in msg:
            raise HTTPException(409, "A catalog source with this name or engine_catalog already exists")
        raise HTTPException(500, "Failed to create catalog source")

    row = db.query_one(
        f"SELECT {_FIELDS} FROM catalog_sources WHERE name = @param0 AND created_by = @param1 "
        "ORDER BY created_at DESC LIMIT 1",
        [name, ctx.email],
    )
    _audit("created", row["id"], name, ctx.email, json.dumps({
        "storage_type": storage_type, "data_format": data_format, "adapter_type": adapter_type,
    }))
    return {"success": True, "catalogSource": row}


@router.patch("/catalog-sources/{cs_id}")
def update_catalog_source(cs_id: str, data: dict, ctx: UserContext = Depends(require_min_role("Admin"))):
    existing = db.query_one("SELECT id, name, lifecycle FROM catalog_sources WHERE id = @param0", [cs_id])
    if not existing:
        raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})

    updates, params, i = [], [], 0

    for field, col, validator in [
        ("name", "name", None),
        ("engine_catalog", "engine_catalog", None),
        ("description", "description", None),
    ]:
        if field in data:
            updates.append(f"{col} = @param{i}")
            params.append((data[field] or "").strip() or None)
            i += 1

    if "storage_type" in data:
        st = (data["storage_type"] or "").strip()
        if st not in _VALID_STORAGE:
            raise HTTPException(400, f"storage_type must be one of: {', '.join(sorted(_VALID_STORAGE))}")
        updates.append(f"storage_type = @param{i}")
        params.append(st)
        i += 1

    if "storage_config" in data:
        sc = data["storage_config"]
        if isinstance(sc, dict):
            sc = json.dumps(sc)
        _validate_json(sc, "storage_config")
        updates.append(f"storage_config = @param{i}")
        params.append(sc)
        i += 1

    if "data_format" in data:
        df = (data["data_format"] or "").strip()
        if df not in _VALID_FORMAT:
            raise HTTPException(400, f"data_format must be one of: {', '.join(sorted(_VALID_FORMAT))}")
        updates.append(f"data_format = @param{i}")
        params.append(df)
        i += 1

    if "credential_kind" in data:
        ck = (data["credential_kind"] or "").strip() or None
        if ck and ck not in _VALID_CREDENTIAL:
            raise HTTPException(400, f"credential_kind must be one of: {', '.join(sorted(_VALID_CREDENTIAL))}")
        updates.append(f"credential_kind = @param{i}")
        params.append(ck)
        i += 1

    if "credential_ref" in data:
        updates.append(f"credential_ref = @param{i}")
        params.append((data["credential_ref"] or "").strip() or None)
        i += 1

    if "adapter_type" in data:
        at = (data["adapter_type"] or "").strip()
        if at not in _VALID_ADAPTER:
            raise HTTPException(400, f"adapter_type must be one of: {', '.join(sorted(_VALID_ADAPTER))}")
        updates.append(f"adapter_type = @param{i}")
        params.append(at)
        i += 1

    if "adapter_config" in data:
        ac = data["adapter_config"]
        if isinstance(ac, dict):
            ac = json.dumps(ac)
        _validate_json(ac, "adapter_config")
        updates.append(f"adapter_config = @param{i}")
        params.append(ac)
        i += 1

    if not updates:
        raise HTTPException(400, "No fields to update")

    updates.append(f"modified_by = @param{i}")
    params.append(ctx.email)
    i += 1
    updates.append("modified_at = NOW()")
    params.append(cs_id)

    try:
        count = db.execute(
            f"UPDATE catalog_sources SET {', '.join(updates)} WHERE id = @param{i}",
            params,
        )
        if not count:
            raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})
    except HTTPException:
        raise
    except Exception as e:
        msg = str(e).lower()
        if "unique" in msg or "duplicate" in msg:
            raise HTTPException(409, "A catalog source with this name or engine_catalog already exists")
        raise HTTPException(500, "Failed to update catalog source")

    updated = db.query_one(f"SELECT {_FIELDS} FROM catalog_sources WHERE id = @param0", [cs_id])
    _audit("updated", cs_id, updated["name"], ctx.email)
    return {"success": True, "catalogSource": updated}


@router.post("/catalog-sources/{cs_id}/transition")
def transition_lifecycle(cs_id: str, data: dict, ctx: UserContext = Depends(require_min_role("Admin"))):
    target = (data.get("lifecycle") or "").strip()
    if target not in _VALID_LIFECYCLE:
        raise HTTPException(400, f"lifecycle must be one of: {', '.join(sorted(_VALID_LIFECYCLE))}")

    existing = db.query_one("SELECT id, name, lifecycle FROM catalog_sources WHERE id = @param0", [cs_id])
    if not existing:
        raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})

    current = existing["lifecycle"]
    allowed = _TRANSITIONS.get(current, set())
    if target not in allowed:
        raise HTTPException(
            400,
            f"Cannot transition from '{current}' to '{target}'. "
            f"Allowed transitions: {', '.join(sorted(allowed)) or 'none'}",
        )

    db.execute(
        "UPDATE catalog_sources SET lifecycle = @param0, modified_by = @param1, modified_at = NOW() "
        "WHERE id = @param2",
        [target, ctx.email, cs_id],
    )
    _audit("lifecycle_transition", cs_id, existing["name"], ctx.email,
           json.dumps({"from": current, "to": target}))

    updated = db.query_one(f"SELECT {_FIELDS} FROM catalog_sources WHERE id = @param0", [cs_id])
    return {"success": True, "catalogSource": updated}


@router.delete("/catalog-sources/{cs_id}")
def delete_catalog_source(cs_id: str, ctx: UserContext = Depends(require_min_role("Admin"))):
    existing = db.query_one("SELECT id, name, lifecycle FROM catalog_sources WHERE id = @param0", [cs_id])
    if not existing:
        raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})

    if existing["lifecycle"] not in ("draft", "deleted"):
        raise HTTPException(
            400,
            "Only draft or already-deleted catalog sources can be permanently removed. "
            "Transition to 'deleting' first for active/suspended sources.",
        )

    db.execute("DELETE FROM catalog_sources WHERE id = @param0", [cs_id])
    _audit("deleted", cs_id, existing["name"], ctx.email)
    return {"success": True, "message": "Catalog source deleted"}


@router.get("/catalog-sources/{cs_id}/audit")
def get_audit_trail(cs_id: str, ctx: UserContext = Depends(require_min_role("Viewer"))):
    existing = db.query_one("SELECT id FROM catalog_sources WHERE id = @param0", [cs_id])
    if not existing:
        raise HTTPException(404, {"code": "NOT_FOUND", "message": "Catalog source not found"})

    result = db.query(
        "SELECT id, action, object_name, timestamp, user_email, details "
        "FROM activity WHERE object_type = 'catalog_source' AND object_id = @param0 "
        "ORDER BY timestamp DESC LIMIT 50",
        [cs_id],
    )
    return {"success": True, "events": result["rows"]}
