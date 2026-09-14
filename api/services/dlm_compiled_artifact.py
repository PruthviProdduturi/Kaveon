"""Immutable compiled-DLM publication and PostgreSQL-free retrieval."""

import hashlib
import json
import os
import re

from services import adls_artifact_client, product_outbox, product_store


MAX_BYTES = 16 * 1024 * 1024
LIVE_PUBLISH_KEY = "KAVEON_DLM_LIVE_ARTIFACT_PUBLISH_ENABLED"
_FIELDS = frozenset({"dataset_id", "version", "manifest", "stats_rollup", "usage_rollup",
                     "source_hash", "built_at", "status", "values_indexed"})
_RETIREMENT_FIELDS = _FIELDS | {"compiled_context"}


def _valid_fields(payload: dict) -> bool:
    return set(payload) in (_FIELDS, _RETIREMENT_FIELDS)


def _canonical(value: dict) -> bytes:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_BYTES:
        raise RuntimeError("Compiled DLM artifact exceeds its byte bound")
    return encoded


def _client():
    return adls_artifact_client.AzureArtifactClient.from_env()


def publish(payload: dict) -> dict | None:
    """Create and reconcile immutable bytes; disabled mode preserves legacy builds."""
    if os.getenv(LIVE_PUBLISH_KEY) != "true":
        return None
    if not _valid_fields(payload) or payload.get("status") != "ready":
        raise RuntimeError("Compiled DLM artifact payload is invalid")
    context = payload.get("compiled_context")
    if context is not None and (not isinstance(context, dict)
            or set(context) != {"values", "answers", "sketches", "router", "curation"}
            or not isinstance(context["values"], list) or not isinstance(context["answers"], list)
            or not isinstance(context["sketches"], list) or not isinstance(context["router"], dict)
            or not isinstance(context["curation"], dict)):
        raise RuntimeError("Compiled DLM context payload is invalid")
    dataset_id, version = str(payload.get("dataset_id") or ""), payload.get("version")
    if not dataset_id.isdecimal() or type(version) is not int or version < 1:
        raise RuntimeError("Compiled DLM artifact identity is invalid")
    content = _canonical(payload)
    digest = hashlib.sha256(content).hexdigest()
    path = f"dlm/{dataset_id}/v{version}/compiled.json"
    client = _client()
    try:
        client.create_if_absent(path, content)
    except Exception:
        if client.read(path, MAX_BYTES + 1) != content:
            raise
    if client.read(path, MAX_BYTES + 1) != content:
        raise RuntimeError("Compiled DLM artifact failed exact publication reconciliation")
    return {"path": path, "sha256": digest, "bytes": len(content), "version": version}


def enqueue_run(transaction, dataset_id: str, owner: str, actor: str,
                definition_revision: int, artifact: dict) -> None:
    record_id = f"{dataset_id}-v{artifact['version']}"
    product_outbox.enqueue(transaction, family="dlm_runs", operation="create",
        record_id=record_id, payload={"definition_id": dataset_id,
            "definition_revision": definition_revision, "status": "ready",
            "artifact": {"path": artifact["path"], "sha256": artifact["sha256"]}},
        actor=actor, owner=owner)


def read(dataset_id: str, actor: str, role: str) -> dict | None:
    """Resolve and verify the newest compiled run without a PostgreSQL fallback."""
    dataset_id = str(dataset_id or "")
    if not dataset_id.isdecimal() or not actor:
        raise RuntimeError("Compiled DLM read identity is invalid")
    if role not in {"Viewer", "Analyst", "Editor", "Admin"}:
        raise RuntimeError("Compiled DLM application role is invalid")
    # The bridge uses its server-held admin credential, then applies the same
    # product visibility rules as dataset routes. Passing Viewer/Analyst to the
    # storage layer would incorrectly reduce reads to owner-only records.
    dataset = product_store.read("dataset", dataset_id, actor, "Admin")
    if dataset is None:
        return None
    dataset_document = dataset.get("document")
    owner = dataset_document.get("created_by") if isinstance(dataset_document, dict) else None
    dataset_revision = dataset.get("revision")
    if not owner or type(dataset_revision) is not int or dataset_revision < 1:
        raise RuntimeError("Compiled DLM dataset authority is invalid")
    visibility = dataset_document.get("visibility") or "internal"
    if not (role == "Admin" or visibility == "published"
            or visibility == "internal" and role in {"Analyst", "Editor"}
            or visibility == "private" and owner == actor):
        return None
    definition = product_store.read("dlm_definition", dataset_id, actor, "Admin")
    expected_definition = {"dataset_id": dataset_id, "dataset_revision": dataset_revision}
    if not definition or definition.get("document") != expected_definition:
        raise RuntimeError("Compiled DLM definition is missing or stale")
    definition_revision = definition.get("revision")
    if type(definition_revision) is not int or definition_revision < 1:
        raise RuntimeError("Compiled DLM definition revision is invalid")
    candidates = []
    for record in product_store.list_records("dlm_run", actor, "Admin", max_records=1000):
        document = record.get("document") if isinstance(record, dict) else None
        match = re.fullmatch(re.escape(dataset_id) + r"-v([1-9][0-9]*)", str(record.get("id") or ""))
        if (match and isinstance(document, dict) and document.get("definition_id") == dataset_id
                and document.get("definition_revision") == definition_revision
                and document.get("status") == "ready"):
            candidates.append((int(match.group(1)), document))
    if not candidates:
        return None
    version, run = max(candidates, key=lambda item: item[0])
    artifact = run.get("artifact")
    expected_path = f"dlm/{dataset_id}/v{version}/compiled.json"
    if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
            or artifact.get("path") != expected_path
            or re.fullmatch(r"[0-9a-f]{64}", str(artifact.get("sha256") or "")) is None):
        raise RuntimeError("Compiled DLM run artifact reference is invalid")
    content = _client().read(expected_path, MAX_BYTES + 1)
    if content is None or len(content) > MAX_BYTES or hashlib.sha256(content).hexdigest() != artifact["sha256"]:
        raise RuntimeError("Compiled DLM artifact is missing or corrupt")
    try:
        payload = json.loads(content)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("Compiled DLM artifact JSON is invalid") from error
    if (not isinstance(payload, dict) or not _valid_fields(payload)
            or payload.get("dataset_id") != dataset_id or payload.get("version") != version
            or payload.get("status") != "ready" or not isinstance(payload.get("manifest"), dict)
            or not isinstance(payload.get("stats_rollup"), dict)
            or not isinstance(payload.get("usage_rollup"), dict)):
        raise RuntimeError("Compiled DLM artifact content is invalid")
    return payload
