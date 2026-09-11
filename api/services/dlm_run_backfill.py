"""Deterministic PostgreSQL DLM artifact to KaveonDB run reconciliation."""

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

from fastapi import HTTPException

import database.metadata as db
from services import product_store


MAX_DLM_RUNS = 10_000


@dataclass(frozen=True)
class RunRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class RunSnapshot:
    source_watermark: int
    definition_snapshot_id: str
    records: tuple[RunRecord, ...]
    snapshot_sha256: str


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def snapshot_digest(records: tuple[RunRecord, ...], definition_snapshot_id: str) -> str:
    digest = hashlib.sha256()
    for value in (definition_snapshot_id, *(item for record in records for item in
                  (record.record_id, record.owner_principal, record.payload_sha256))):
        encoded = value.encode()
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def validate_snapshot(snapshot: RunSnapshot) -> None:
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_DLM_RUNS:
        raise RuntimeError("DLM run snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda item: item.record_id)):
        raise RuntimeError("DLM run snapshot order is invalid")
    for record in snapshot.records:
        document = record.document
        if hashlib.sha256(_canonical(document)).hexdigest() != record.payload_sha256:
            raise RuntimeError(f"DLM run {record.record_id} identity is invalid")
        if set(document) != {"definition_id", "definition_revision", "status", "artifact"}:
            raise RuntimeError(f"DLM run {record.record_id} document is invalid")
        artifact = document.get("artifact")
        if (document.get("status") != "ready" or type(document.get("definition_revision")) is not int
                or document["definition_revision"] < 1 or not isinstance(artifact, dict)
                or set(artifact) != {"path", "sha256"}
                or re.fullmatch(r"[0-9a-f]{64}", artifact.get("sha256", "")) is None
                or not isinstance(artifact.get("path"), str)
                or not artifact["path"].startswith("dlm/") or ".." in artifact["path"].split("/")):
            raise RuntimeError(f"DLM run {record.record_id} document is invalid")
    if snapshot_digest(snapshot.records, snapshot.definition_snapshot_id) != snapshot.snapshot_sha256:
        raise RuntimeError("DLM run snapshot identity mismatch")


def capture_snapshot(artifact_root: Path) -> RunSnapshot:
    """Capture only ready legacy artifacts whose exact canonical bytes are staged locally."""
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox"
        ) or {}
        rows = transaction.query("""
            SELECT d.id, d.created_by, a.version, a.manifest, a.status
            FROM datasets d JOIN dlm_artifact a ON a.dataset_id = CAST(d.id AS TEXT)
            ORDER BY d.id, a.version LIMIT @param0
        """, [MAX_DLM_RUNS + 1])["rows"]
    if len(rows) > MAX_DLM_RUNS:
        raise RuntimeError("DLM run snapshot exceeds its record bound")
    records, snapshot_id = [], None
    for row in rows:
        dataset_id, owner = str(row["id"]), str(row["created_by"])
        version = row.get("version")
        if not dataset_id.isdecimal() or type(version) is not int or version < 1 or row.get("status") != "ready":
            raise RuntimeError(f"PostgreSQL DLM artifact {dataset_id} is unsupported")
        try:
            manifest = json.loads(row["manifest"])
        except (TypeError, json.JSONDecodeError) as error:
            raise RuntimeError(f"PostgreSQL DLM artifact {dataset_id} manifest is invalid") from error
        artifact_bytes = _canonical(manifest)
        relative_path = f"dlm/{dataset_id}/v{version}/manifest.json"
        staged = artifact_root.resolve() / Path(relative_path)
        if not staged.is_file() or staged.read_bytes() != artifact_bytes:
            raise RuntimeError(f"Staged DLM artifact {dataset_id} is missing or divergent")
        definition = product_store.read("dlm_definition", dataset_id, owner, "Admin")
        if definition is None:
            raise RuntimeError(f"KaveonDB DLM definition {dataset_id} is missing")
        current_snapshot, revision = str(definition.get("snapshot_id") or ""), definition.get("revision")
        if (not current_snapshot or (snapshot_id is not None and snapshot_id != current_snapshot)
                or type(revision) is not int or revision < 1):
            raise RuntimeError("KaveonDB DLM definition snapshot or revision is invalid")
        snapshot_id = current_snapshot
        record_id = f"{dataset_id}-v{version}"
        document = {"definition_id": dataset_id, "definition_revision": revision, "status": "ready",
                    "artifact": {"path": relative_path,
                                 "sha256": hashlib.sha256(artifact_bytes).hexdigest()}}
        records.append(RunRecord(record_id, owner, document,
                                 hashlib.sha256(_canonical(document)).hexdigest()))
    immutable = tuple(records)
    snapshot_id = snapshot_id or "empty"
    return RunSnapshot(int(watermark.get("watermark") or 0), snapshot_id, immutable,
                       snapshot_digest(immutable, snapshot_id))


def apply_and_reconcile(snapshot: RunSnapshot) -> dict:
    validate_snapshot(snapshot)
    created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("dlm_run", record.record_id, record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document:
            already_present += 1
            continue
        if target is not None:
            raise RuntimeError(f"KaveonDB DLM run {record.record_id} diverges")
        building = {**record.document, "status": "building", "artifact": None}
        try:
            product_store.transact([
                product_store.ProductMutation("create", "dlm_run", record.record_id, building),
                product_store.ProductMutation("update", "dlm_run", record.record_id,
                                              record.document, expected_revision=1),
            ], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read("dlm_run", record.record_id, record.owner_principal, "Admin")
            if error.status_code != 409 or resolved is None or resolved.get("document") != record.document:
                raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("dlm_run", record.record_id, record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB DLM run {record.record_id} failed reconciliation")
    return {"family": "dlm_runs", "source_watermark": snapshot.source_watermark,
            "definition_snapshot_id": snapshot.definition_snapshot_id,
            "source_count": len(snapshot.records), "created": created,
            "already_present": already_present, "reconciled": len(snapshot.records),
            "snapshot_sha256": snapshot.snapshot_sha256}
