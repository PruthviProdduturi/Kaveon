"""Deterministic PostgreSQL chart snapshot and KaveonDB reconciliation."""

import hashlib
import json
from dataclasses import dataclass

from fastapi import HTTPException

import database.metadata as db
from services import product_store

MAX_CHARTS = 10_000
MAX_DOCUMENT_BYTES = 1024 * 1024


@dataclass(frozen=True)
class ChartRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class ChartSnapshot:
    source_watermark: int
    dataset_snapshot_id: str
    records: tuple[ChartRecord, ...]
    snapshot_sha256: str


def _canonical(document: dict) -> tuple[bytes, str]:
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    if len(encoded) > MAX_DOCUMENT_BYTES:
        raise RuntimeError("chart document exceeds its byte bound")
    return encoded, hashlib.sha256(encoded).hexdigest()


def snapshot_digest(records, dataset_snapshot_id):
    digest = hashlib.sha256()
    for value in (dataset_snapshot_id, *(part for record in records for part in
                  (record.record_id, record.owner_principal, record.payload_sha256))):
        encoded = value.encode(); digest.update(len(encoded).to_bytes(8, "big")); digest.update(encoded)
    return digest.hexdigest()


def _json(value, label):
    try:
        result = json.loads(value or "{}")
    except (TypeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"chart {label} is invalid") from error
    if not isinstance(result, dict):
        raise RuntimeError(f"chart {label} is invalid")
    return result


def _text(value):
    return value.isoformat() if hasattr(value, "isoformat") else value


def _document(row, layout, dataset_revision):
    if layout == "modern":
        envelope = _json(row.get("config"), "config")
        query = envelope.get("query_config", envelope)
        viz = envelope.get("viz_config", {})
        if not isinstance(query, dict) or not isinstance(viz, dict):
            raise RuntimeError("chart config is invalid")
        modified_by, updated_at = row.get("modified_by"), row.get("modified_at")
    else:
        query, viz = _json(row.get("query_config"), "query_config"), _json(row.get("viz_config"), "viz_config")
        modified_by, updated_at = row.get("updated_by"), row.get("updated_at")
    dataset_id = str(row.get("dataset_id") or query.get("dataset_id") or "")
    if not dataset_id:
        raise RuntimeError("chart dataset_id is missing")
    visibility = row.get("visibility") or "internal"
    if visibility not in {"private", "internal", "published"}:
        raise RuntimeError("chart visibility is invalid")
    return {"id": str(row["id"]), "name": row.get("name"), "description": row.get("description"),
            "dataset_id": dataset_id, "dataset_revision": dataset_revision,
            "chart_type": row.get("chart_type") or "table", "query_config": query,
            "viz_config": viz, "visibility": visibility, "created_at": _text(row.get("created_at")),
            "updated_at": _text(updated_at), "created_by": str(row.get("created_by") or ""),
            "modified_by": str(modified_by or row.get("created_by") or "")}


def validate_snapshot(snapshot):
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_CHARTS:
        raise RuntimeError("chart snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda record: record.record_id)):
        raise RuntimeError("chart snapshot order is invalid")
    for record in snapshot.records:
        _, digest = _canonical(record.document)
        if digest != record.payload_sha256 or record.document.get("id") != record.record_id \
                or not record.owner_principal or record.document.get("created_by") != record.owner_principal \
                or type(record.document.get("dataset_revision")) is not int \
                or record.document["dataset_revision"] < 1:
            raise RuntimeError(f"chart {record.record_id} is invalid")
    if snapshot_digest(snapshot.records, snapshot.dataset_snapshot_id) != snapshot.snapshot_sha256:
        raise RuntimeError("chart snapshot identity mismatch")


def capture_snapshot():
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox") or {}
        columns = transaction.query(
            "SELECT column_name FROM information_schema.columns WHERE table_name = @param0", ["charts"])["rows"]
        names = {str(row["column_name"]).casefold() for row in columns}
        if {"dataset_id", "config", "modified_at", "modified_by"} <= names:
            layout = "modern"
            sql = """SELECT id,name,description,dataset_id,chart_type,config,visibility,created_by,
                     modified_by,created_at,modified_at FROM charts ORDER BY id LIMIT @param0"""
        elif {"query_config", "viz_config", "updated_at", "updated_by"} <= names:
            layout = "legacy"
            sql = """SELECT id,name,description,chart_type,query_config,viz_config,visibility,created_by,
                     updated_by,created_at,updated_at FROM charts ORDER BY id LIMIT @param0"""
        else:
            raise RuntimeError("charts metadata schema is unsupported")
        rows = transaction.query(sql, [MAX_CHARTS + 1])["rows"]
    if len(rows) > MAX_CHARTS:
        raise RuntimeError("chart snapshot exceeds its record bound")
    records, snapshot_id = [], None
    for row in rows:
        preliminary = _document(row, layout, 1)
        dataset_id, owner = preliminary["dataset_id"], preliminary["created_by"]
        if not owner:
            raise RuntimeError(f"chart {row['id']} owner is missing")
        dataset = product_store.read("dataset", dataset_id, owner, "Admin")
        if dataset is None:
            raise RuntimeError(f"KaveonDB dataset {dataset_id} is missing for chart")
        current_snapshot, revision = str(dataset.get("snapshot_id") or ""), dataset.get("revision")
        if (not current_snapshot or (snapshot_id is not None and current_snapshot != snapshot_id)
                or type(revision) is not int or revision < 1):
            raise RuntimeError("KaveonDB dataset snapshot or revision is invalid")
        snapshot_id = current_snapshot
        document = _document(row, layout, revision)
        _, payload_hash = _canonical(document)
        records.append(ChartRecord(str(row["id"]), owner, document, payload_hash))
    records = tuple(sorted(records, key=lambda record: record.record_id))
    snapshot_id = snapshot_id or "empty"
    return ChartSnapshot(int(watermark.get("watermark") or 0), snapshot_id, records,
                         snapshot_digest(records, snapshot_id))


def apply_and_reconcile(snapshot):
    validate_snapshot(snapshot)
    created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("chart", record.record_id, record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document:
            already_present += 1; continue
        if target is not None:
            raise RuntimeError(f"KaveonDB chart {record.record_id} diverges")
        try:
            product_store.transact([product_store.ProductMutation(
                "create", "chart", record.record_id, record.document)], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read("chart", record.record_id, record.owner_principal, "Admin")
            if error.status_code != 409 or resolved is None or resolved.get("document") != record.document:
                raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("chart", record.record_id, record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB chart {record.record_id} failed reconciliation")
    return {"family": "charts", "source_watermark": snapshot.source_watermark,
            "dataset_snapshot_id": snapshot.dataset_snapshot_id, "source_count": len(snapshot.records),
            "created": created, "already_present": already_present, "reconciled": len(snapshot.records),
            "snapshot_sha256": snapshot.snapshot_sha256}
