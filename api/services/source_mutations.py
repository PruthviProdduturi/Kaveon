"""Canonical, non-secret source outbox documents."""
import json
from services import product_outbox


def catalog_document(row: dict) -> dict:
    raw_ref = row.get("credential_ref")
    kind = str(row.get("credential_kind") or ("secret_store" if raw_ref else "managed_identity"))
    if kind in {"managed_identity", "workload_identity"}:
        if raw_ref and (len(str(raw_ref)) > 255 or any(word in str(raw_ref).casefold() for word in ("password", "secret", "token"))):
            raise ValueError("catalog identity reference is invalid")
        ref = f"identity:{raw_ref or kind}"
    else:
        ref = raw_ref
        if not ref or not (str(ref).startswith("https://") and ".vault.azure.net/" in str(ref)):
            raise ValueError("catalog credential_ref must be a Key Vault URI")
    def object_value(name):
        value = row.get(name) or "{}"
        if isinstance(value, dict): return value
        parsed = json.loads(value)
        if not isinstance(parsed, dict): raise ValueError(f"catalog {name} must be an object")
        return parsed
    return {"source_kind":"catalog","source_id":f"catalog-{row['id']}","name":row.get("name"),
            "catalog_identity":row.get("engine_catalog"),"source_type":row.get("storage_type"),
            "database_name":None,"region":None,"description":row.get("description"),
            "is_active":row.get("lifecycle")=="active","lifecycle":row.get("lifecycle"),"secret_ref":str(ref),
            "storage_config":object_value("storage_config"),"data_format":row.get("data_format") or "parquet",
            "credential_kind":row.get("credential_kind"),
            "credential_ref":str(raw_ref) if raw_ref else None,
            "adapter_type":row.get("adapter_type") or "native","adapter_config":object_value("adapter_config"),
            "created_by":row.get("created_by"),"modified_by":row.get("modified_by") or row.get("created_by"),
            "created_at":row.get("created_at").isoformat() if hasattr(row.get("created_at"),"isoformat") else row.get("created_at"),
            "modified_at":row.get("modified_at").isoformat() if hasattr(row.get("modified_at"),"isoformat") else row.get("modified_at")}


def data_document(row: dict) -> dict:
    return {"source_kind":"data","source_id":f"data-{row['id']}","name":row.get("name"),
            "catalog_identity":row.get("database_name"),"source_type":row.get("type"),
            "database_name":row.get("database_name"),"region":row.get("region"),
            "description":row.get("description"),"is_active":bool(row.get("is_active")),
            "lifecycle":"active" if row.get("is_active") else "suspended",
            "secret_ref":row.get("secret_ref") or f"key-managed:data_sources/{row['id']}",
            "created_by":row.get("created_by"),"modified_by":row.get("modified_by") or row.get("created_by"),
            "created_at":row.get("created_at"),"modified_at":row.get("modified_at")}


def enqueue(transaction, family: str, operation: str, row: dict, actor: str):
    document = catalog_document(row) if family == "catalog_sources" else data_document(row)
    return product_outbox.enqueue(transaction, family=family, operation=operation,
        record_id=document["source_id"], payload={} if operation == "delete" else document,
        actor=actor, owner=str(row.get("created_by") or actor))
