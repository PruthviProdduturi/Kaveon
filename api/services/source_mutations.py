"""Canonical, non-secret source outbox documents."""
from services import product_outbox


def catalog_document(row: dict) -> dict:
    ref = row.get("credential_ref") or f"identity:{row.get('credential_kind') or 'managed_identity'}"
    if row.get("credential_ref") and not (str(ref).startswith("https://") and ".vault.azure.net/" in str(ref)):
        raise ValueError("catalog credential_ref must be a Key Vault URI")
    return {"source_kind":"catalog","source_id":f"catalog:{row['id']}","name":row.get("name"),
            "catalog_identity":row.get("engine_catalog"),"source_type":row.get("storage_type"),
            "database_name":None,"region":None,"description":row.get("description"),
            "is_active":row.get("lifecycle")=="active","lifecycle":row.get("lifecycle"),"secret_ref":str(ref)}


def data_document(row: dict) -> dict:
    return {"source_kind":"data","source_id":f"data:{row['id']}","name":row.get("name"),
            "catalog_identity":row.get("database_name"),"source_type":row.get("type"),
            "database_name":row.get("database_name"),"region":row.get("region"),
            "description":row.get("description"),"is_active":bool(row.get("is_active")),
            "lifecycle":"active" if row.get("is_active") else "suspended",
            "secret_ref":f"key-managed:data_sources/{row['id']}"}


def enqueue(transaction, family: str, operation: str, row: dict, actor: str):
    document = catalog_document(row) if family == "catalog_sources" else data_document(row)
    prefix = "catalog" if family == "catalog_sources" else "data"
    return product_outbox.enqueue(transaction, family=family, operation=operation,
        record_id=f"{prefix}:{row['id']}", payload={} if operation == "delete" else document,
        actor=actor, owner=str(row.get("created_by") or actor))
