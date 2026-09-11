"""Deterministic ready-DLM definition snapshot and target reconciliation."""

import hashlib
import json
from dataclasses import dataclass

from fastapi import HTTPException

import database.metadata as db
from services import product_store


MAX_DLM_DEFINITIONS = 10_000


@dataclass(frozen=True)
class DefinitionRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class DefinitionSnapshot:
    source_watermark: int
    dataset_snapshot_id: str
    records: tuple[DefinitionRecord, ...]
    snapshot_sha256: str


def _canonical(value: dict) -> tuple[str, str]:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return encoded, hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def snapshot_digest(records: tuple[DefinitionRecord, ...], dataset_snapshot_id: str) -> str:
    digest = hashlib.sha256()
    for value in (dataset_snapshot_id, *(
        item for record in records
        for item in (record.record_id, record.owner_principal, record.payload_sha256)
    )):
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def validate_snapshot(snapshot: DefinitionSnapshot) -> None:
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_DLM_DEFINITIONS:
        raise RuntimeError("DLM definition snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda item: int(item.record_id))):
        raise RuntimeError("DLM definition snapshot order is invalid")
    for record in snapshot.records:
        _, payload_hash = _canonical(record.document)
        if payload_hash != record.payload_sha256 or record.document != {
            "dataset_id": record.record_id,
            "dataset_revision": record.document.get("dataset_revision"),
        } or type(record.document["dataset_revision"]) is not int or record.document["dataset_revision"] < 1:
            raise RuntimeError(f"DLM definition {record.record_id} is invalid")
    if snapshot_digest(snapshot.records, snapshot.dataset_snapshot_id) != snapshot.snapshot_sha256:
        raise RuntimeError("DLM definition snapshot identity mismatch")


def capture_snapshot() -> DefinitionSnapshot:
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox"
        ) or {}
        rows = transaction.query("""
            SELECT d.id, d.created_by
            FROM datasets d
            JOIN dlm_artifact a ON a.dataset_id = CAST(d.id AS TEXT)
            WHERE a.status = 'ready'
            ORDER BY d.id LIMIT @param0
        """, [MAX_DLM_DEFINITIONS + 1])["rows"]
    if len(rows) > MAX_DLM_DEFINITIONS:
        raise RuntimeError("DLM definition snapshot exceeds its record bound")
    records, snapshot_id = [], None
    for row in rows:
        record_id, owner = str(row["id"]), str(row["created_by"])
        dataset = product_store.read("dataset", record_id, owner, "Admin")
        if dataset is None:
            raise RuntimeError(f"KaveonDB dataset {record_id} is missing for DLM definition")
        current_snapshot = str(dataset.get("snapshot_id") or "")
        if not current_snapshot or (snapshot_id is not None and snapshot_id != current_snapshot):
            raise RuntimeError("KaveonDB dataset snapshot changed during DLM definition capture")
        snapshot_id = current_snapshot
        revision = dataset.get("revision")
        if type(revision) is not int or revision < 1:
            raise RuntimeError(f"KaveonDB dataset {record_id} revision is invalid")
        document = {"dataset_id": record_id, "dataset_revision": revision}
        _, payload_hash = _canonical(document)
        records.append(DefinitionRecord(record_id, owner, document, payload_hash))
    immutable = tuple(records)
    snapshot_id = snapshot_id or "empty"
    return DefinitionSnapshot(
        int(watermark.get("watermark") or 0), snapshot_id, immutable,
        snapshot_digest(immutable, snapshot_id),
    )


def apply_and_reconcile(snapshot: DefinitionSnapshot) -> dict:
    validate_snapshot(snapshot)
    created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("dlm_definition", record.record_id, record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document:
            already_present += 1
            continue
        if target is not None:
            raise RuntimeError(f"KaveonDB DLM definition {record.record_id} diverges")
        try:
            product_store.transact([
                product_store.ProductMutation("create", "dlm_definition", record.record_id, record.document)
            ], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read(
                "dlm_definition", record.record_id, record.owner_principal, "Admin"
            )
            if error.status_code != 409 or resolved is None or resolved.get("document") != record.document:
                raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("dlm_definition", record.record_id, record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB DLM definition {record.record_id} failed reconciliation")
    return {
        "family": "dlm_definitions", "source_watermark": snapshot.source_watermark,
        "dataset_snapshot_id": snapshot.dataset_snapshot_id,
        "source_count": len(snapshot.records), "created": created,
        "already_present": already_present, "reconciled": len(snapshot.records),
        "snapshot_sha256": snapshot.snapshot_sha256,
    }
