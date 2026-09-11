"""Disabled, telemetry-only comparison of PostgreSQL and KaveonDB product reads."""

import hashlib
import json
import os

from services import product_store


MAX_SHADOW_DOCUMENT_BYTES = 1024 * 1024
MAX_SHADOW_LIST_RECORDS = 25
CHART_SHADOW_FIELDS = (
    "id", "name", "description", "dataset_id", "chart_type", "query_config",
    "viz_config", "visibility", "created_at", "updated_at", "created_by", "modified_by",
)
DASHBOARD_SHADOW_FIELDS = (
    "id", "name", "description", "layout", "charts", "filters", "theme", "visibility",
    "is_published", "is_archived", "created_at", "updated_at", "created_by", "modified_by",
)


def _identity(document: dict) -> tuple[str, int]:
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_SHADOW_DOCUMENT_BYTES:
        raise RuntimeError("dataset shadow document exceeds its byte bound")
    return hashlib.sha256(encoded).hexdigest(), len(encoded)


def compare_dataset(source_document: dict, actor: str, role: str) -> dict:
    """Point-read the target as the requesting principal and report parity only."""
    if os.getenv("KAVEON_DATASET_SHADOW_READ_ENABLED") != "true":
        return {"family": "datasets", "enabled": False, "status": "disabled"}
    record_id = str(source_document.get("id") or "")
    if not record_id or not actor:
        raise RuntimeError("dataset shadow comparison requires record and actor identity")
    source_sha, source_bytes = _identity(source_document)
    target = product_store.read("dataset", record_id, actor, role)
    if target is None:
        return {
            "family": "datasets", "enabled": True, "record_id": record_id,
            "status": "missing", "source_sha256": source_sha,
            "source_bytes": source_bytes, "target_sha256": None,
        }
    target_document = target.get("document")
    if not isinstance(target_document, dict):
        raise RuntimeError("KaveonDB dataset shadow response is invalid")
    target_sha, target_bytes = _identity(target_document)
    return {
        "family": "datasets", "enabled": True, "record_id": record_id,
        "status": "match" if target_sha == source_sha else "mismatch",
        "source_sha256": source_sha, "target_sha256": target_sha,
        "source_bytes": source_bytes, "target_bytes": target_bytes,
        "target_generation": int(target.get("generation") or 0),
    }


def compare_dataset_list(source_documents: list[dict], actor: str, role: str) -> dict:
    """Compare the PostgreSQL list projection through bounded owner-scoped reads."""
    if os.getenv("KAVEON_DATASET_SHADOW_READ_ENABLED") != "true":
        return {"family": "datasets", "operation": "list", "enabled": False, "status": "disabled"}
    if not actor:
        raise RuntimeError("dataset list shadow comparison requires actor identity")
    if len(source_documents) > MAX_SHADOW_LIST_RECORDS:
        return {
            "family": "datasets", "operation": "list", "enabled": True,
            "status": "skipped_limit", "source_count": len(source_documents),
            "limit": MAX_SHADOW_LIST_RECORDS,
        }
    counts = {"match": 0, "missing": 0, "mismatch": 0}
    source_identities, target_identities = [], []
    for source in source_documents:
        projection = {key: value for key, value in source.items() if key != "favorite"}
        record_id = str(projection.get("id") or "")
        if not record_id:
            raise RuntimeError("dataset list shadow comparison requires record identity")
        source_sha, _ = _identity(projection)
        source_identities.append(source_sha)
        target = product_store.read("dataset", record_id, actor, role)
        if target is None:
            counts["missing"] += 1
            target_identities.append("missing")
            continue
        document = target.get("document")
        if not isinstance(document, dict):
            raise RuntimeError("KaveonDB dataset list shadow response is invalid")
        target_projection = {key: document.get(key) for key in projection}
        target_sha, _ = _identity(target_projection)
        target_identities.append(target_sha)
        counts["match" if target_sha == source_sha else "mismatch"] += 1
    batch_source_sha = hashlib.sha256("".join(source_identities).encode("ascii")).hexdigest()
    batch_target_sha = hashlib.sha256("".join(target_identities).encode("ascii")).hexdigest()
    return {
        "family": "datasets", "operation": "list", "enabled": True,
        "status": "match" if counts["match"] == len(source_documents) else "mismatch",
        "source_count": len(source_documents), "compared": len(source_documents),
        **counts, "source_sha256": batch_source_sha, "target_sha256": batch_target_sha,
    }


def compare_chart(source_document: dict, actor: str, role: str) -> dict:
    """Compare a bounded chart projection as the requesting principal."""
    if os.getenv("KAVEON_CHART_SHADOW_READ_ENABLED") != "true":
        return {"family": "charts", "enabled": False, "status": "disabled"}
    record_id = str(source_document.get("id") or "")
    if not record_id or not actor:
        raise RuntimeError("chart shadow comparison requires record and actor identity")
    source_projection = {field: source_document.get(field) for field in CHART_SHADOW_FIELDS}
    source_sha, source_bytes = _identity(source_projection)
    target = product_store.read("chart", record_id, actor, role)
    base = {
        "family": "charts", "enabled": True, "record_id": record_id,
        "source_sha256": source_sha, "source_bytes": source_bytes,
    }
    if target is None:
        return {**base, "status": "missing", "target_sha256": None}
    document = target.get("document")
    if not isinstance(document, dict):
        raise RuntimeError("KaveonDB chart shadow response is invalid")
    target_projection = {field: document.get(field) for field in CHART_SHADOW_FIELDS}
    target_sha, target_bytes = _identity(target_projection)
    return {
        **base, "status": "match" if source_sha == target_sha else "mismatch",
        "target_sha256": target_sha, "target_bytes": target_bytes,
        "target_generation": int(target.get("generation") or 0),
    }


def compare_dashboard(source_document: dict, actor: str, role: str) -> dict:
    """Compare canonical dashboard content without changing its PostgreSQL response."""
    if os.getenv("KAVEON_DASHBOARD_SHADOW_READ_ENABLED") != "true":
        return {"family": "dashboards", "enabled": False, "status": "disabled"}
    record_id = str(source_document.get("id") or "")
    if not record_id or not actor:
        raise RuntimeError("dashboard shadow comparison requires record and actor identity")
    source_projection = {field: source_document.get(field) for field in DASHBOARD_SHADOW_FIELDS}
    for field in ("layout", "charts", "filters"):
        value = source_projection[field]
        try:
            source_projection[field] = json.loads(value) if isinstance(value, str) else value
        except json.JSONDecodeError as error:
            raise RuntimeError(f"dashboard shadow {field} is invalid") from error
    source_sha, source_bytes = _identity(source_projection)
    target = product_store.read("dashboard", record_id, actor, role)
    base = {"family": "dashboards", "enabled": True, "record_id": record_id,
            "source_sha256": source_sha, "source_bytes": source_bytes}
    if target is None:
        return {**base, "status": "missing", "target_sha256": None}
    document = target.get("document")
    if not isinstance(document, dict):
        raise RuntimeError("KaveonDB dashboard shadow response is invalid")
    target_projection = {field: document.get(field) for field in DASHBOARD_SHADOW_FIELDS}
    target_sha, target_bytes = _identity(target_projection)
    return {**base, "status": "match" if source_sha == target_sha else "mismatch",
            "target_sha256": target_sha, "target_bytes": target_bytes,
            "target_generation": int(target.get("generation") or 0)}


def observe_user_theme(source_document: dict, owner: str) -> dict:
    """Compare an owner-keyed theme and return content-free telemetry."""
    if os.getenv("KAVEON_USER_THEME_SHADOW_READ_ENABLED") != "true":
        return {"family": "user_themes", "enabled": False, "status": "disabled"}
    document = {"user_email": owner, "theme_color": source_document.get("theme_color")}
    source_sha, source_bytes = _identity(document)
    target = product_store.read("user_theme", owner, owner, "Viewer")
    base = {"family": "user_themes", "enabled": True, "record_id_sha256":
            hashlib.sha256(owner.encode()).hexdigest(), "source_sha256": source_sha,
            "source_bytes": source_bytes}
    if target is None:
        return {**base, "status": "missing", "target_sha256": None}
    target_document = target.get("document")
    if not isinstance(target_document, dict):
        raise RuntimeError("KaveonDB user theme shadow response is invalid")
    target_sha, target_bytes = _identity(target_document)
    return {**base, "status": "match" if source_sha == target_sha else "mismatch",
            "target_sha256": target_sha, "target_bytes": target_bytes,
            "target_generation": int(target.get("generation") or 0)}


def observe_favorite_list(source_records: list[dict], owner: str) -> dict:
    if os.getenv("KAVEON_FAVORITE_SHADOW_READ_ENABLED") != "true":
        return {"family":"favorites","enabled":False,"status":"disabled"}
    if len(source_records)>MAX_SHADOW_LIST_RECORDS:
        return {"family":"favorites","enabled":True,"status":"skipped_limit","source_count":len(source_records)}
    counts={"match":0,"missing":0,"mismatch":0}
    for source in source_records:
        kind=str(source.get("kind") or "");oid=str(source.get("id") or "")
        record_id=hashlib.sha256(f"{owner}\0{kind}\0{oid}".encode()).hexdigest()
        document={"user_email":owner,"object_type":kind,"object_id":oid,"object_name":source.get("name")}
        target=product_store.read("favorite",record_id,owner,"Viewer")
        if target is None:counts["missing"]+=1
        elif target.get("document")==document:counts["match"]+=1
        else:counts["mismatch"]+=1
    status="match" if counts["match"]==len(source_records) else "mismatch"
    return {"family":"favorites","enabled":True,"status":status,"source_count":len(source_records),**counts}

def observe_user_recent_list(source_records:list[dict],owner:str)->dict:
    if os.getenv("KAVEON_USER_RECENT_SHADOW_READ_ENABLED")!="true":return {"family":"user_recents","enabled":False,"status":"disabled"}
    if len(source_records)>20:return {"family":"user_recents","enabled":True,"status":"skipped_limit","source_count":len(source_records)}
    from services.user_recent_backfill import record_id
    counts={"match":0,"missing":0,"mismatch":0}
    for source in source_records:
        document={"user_email":owner,"item_id":str(source.get("item_id") or ""),"label":source.get("label"),"href":source.get("href"),"type":source.get("type"),"created_at":source.get("created_at").isoformat() if hasattr(source.get("created_at"),"isoformat") else str(source.get("created_at") or "")}
        target=product_store.read("user_recent",record_id(owner,document["item_id"]),owner,"Viewer")
        if target is None:counts["missing"]+=1
        elif target.get("document")==document:counts["match"]+=1
        else:counts["mismatch"]+=1
    return {"family":"user_recents","enabled":True,"status":"match" if counts["match"]==len(source_records) else "mismatch","source_count":len(source_records),**counts}
