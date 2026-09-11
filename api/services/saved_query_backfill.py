"""Deterministic PostgreSQL saved-query snapshot and KaveonDB reconciliation."""

import hashlib
import json
from dataclasses import dataclass

from fastapi import HTTPException

import database.metadata as db
from services import product_store

MAX_SAVED_QUERIES = 25_000
MAX_DOCUMENT_BYTES = 1024 * 1024


@dataclass(frozen=True)
class SavedQueryRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class SavedQuerySnapshot:
    source_watermark: int
    records: tuple[SavedQueryRecord, ...]
    snapshot_sha256: str


def _canonical(document: dict) -> tuple[bytes, str]:
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"),
                         ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_DOCUMENT_BYTES:
        raise RuntimeError("saved-query document exceeds its byte bound")
    return encoded, hashlib.sha256(encoded).hexdigest()


def snapshot_digest(records) -> str:
    digest = hashlib.sha256()
    for value in (part for record in records for part in
                  (record.record_id, record.owner_principal, record.payload_sha256)):
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def _text(value):
    return value.isoformat() if hasattr(value, "isoformat") else value


def _document(row: dict, layout: str) -> dict:
    owner = str(row.get("created_by") or "")
    updated_at = row.get("modified_at") if layout == "extended" else row.get("updated_at")
    modified_by = row.get("modified_by") if layout == "extended" else owner
    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "sql": row.get("sql_text"),
        "created_at": _text(row.get("created_at")),
        "updated_at": _text(updated_at),
        "created_by": owner,
        "modified_by": str(modified_by or owner),
    }


def validate_snapshot(snapshot: SavedQuerySnapshot) -> None:
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_SAVED_QUERIES:
        raise RuntimeError("saved-query snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda record: record.record_id)):
        raise RuntimeError("saved-query snapshot order is invalid")
    for record in snapshot.records:
        _, digest = _canonical(record.document)
        if (digest != record.payload_sha256 or record.document.get("id") != record.record_id
                or not record.owner_principal
                or record.document.get("created_by") != record.owner_principal
                or not isinstance(record.document.get("sql"), str)):
            raise RuntimeError(f"saved query {record.record_id} is invalid")
    if snapshot_digest(snapshot.records) != snapshot.snapshot_sha256:
        raise RuntimeError("saved-query snapshot identity mismatch")


def capture_snapshot() -> SavedQuerySnapshot:
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox") or {}
        columns = transaction.query(
            "SELECT column_name FROM information_schema.columns WHERE table_name = @param0",
            ["saved_queries"],
        )["rows"]
        names = {str(row["column_name"]).casefold() for row in columns}
        required = {"id", "name", "description", "sql_text", "created_by", "created_at"}
        if not required <= names:
            raise RuntimeError("saved_queries metadata schema is unsupported")
        if {"modified_at", "modified_by"} <= names:
            layout = "extended"
            projection = "id,name,description,sql_text,created_by,created_at,modified_by,modified_at"
        elif "updated_at" in names:
            layout = "simple"
            projection = "id,name,description,sql_text,created_by,created_at,updated_at"
        else:
            raise RuntimeError("saved_queries metadata schema is unsupported")
        rows = transaction.query(
            f"SELECT {projection} FROM saved_queries ORDER BY id LIMIT @param0",
            [MAX_SAVED_QUERIES + 1],
        )["rows"]
    if len(rows) > MAX_SAVED_QUERIES:
        raise RuntimeError("saved-query snapshot exceeds its record bound")
    records = []
    for row in rows:
        document = _document(row, layout)
        owner = document["created_by"]
        if not owner:
            raise RuntimeError(f"saved query {row['id']} owner is missing")
        _, payload_hash = _canonical(document)
        records.append(SavedQueryRecord(str(row["id"]), owner, document, payload_hash))
    ordered = tuple(sorted(records, key=lambda record: record.record_id))
    snapshot = SavedQuerySnapshot(int(watermark.get("watermark") or 0), ordered,
                                  snapshot_digest(ordered))
    validate_snapshot(snapshot)
    return snapshot


def apply_and_reconcile(snapshot: SavedQuerySnapshot) -> dict:
    validate_snapshot(snapshot)
    created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("saved_query", record.record_id,
                                    record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document:
            already_present += 1
            continue
        if target is not None:
            raise RuntimeError(f"KaveonDB saved query {record.record_id} diverges")
        try:
            product_store.transact([product_store.ProductMutation(
                "create", "saved_query", record.record_id, record.document,
            )], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read("saved_query", record.record_id,
                                          record.owner_principal, "Admin")
            if (error.status_code != 409 or resolved is None
                    or resolved.get("document") != record.document):
                raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("saved_query", record.record_id,
                                    record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB saved query {record.record_id} failed reconciliation")
    return {
        "family": "saved_queries",
        "source_watermark": snapshot.source_watermark,
        "source_count": len(snapshot.records),
        "created": created,
        "already_present": already_present,
        "reconciled": len(snapshot.records),
        "snapshot_sha256": snapshot.snapshot_sha256,
    }
