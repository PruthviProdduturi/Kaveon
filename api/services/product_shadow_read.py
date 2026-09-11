"""Disabled, telemetry-only comparison of PostgreSQL and KaveonDB product reads."""

import hashlib
import json
import os

from services import product_store


MAX_SHADOW_DOCUMENT_BYTES = 1024 * 1024


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
