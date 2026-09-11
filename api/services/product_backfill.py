"""Deterministic PostgreSQL snapshot and KaveonDB dataset reconciliation."""

import hashlib
import json
from dataclasses import dataclass

from fastapi import HTTPException

import database.metadata as db
from services import datasets, product_store


MAX_DATASETS = 10_000
MAX_COMPONENTS = 1_000_000
MAX_SNAPSHOT_BYTES = 256 * 1024 * 1024


@dataclass(frozen=True)
class SnapshotRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class DatasetSnapshot:
    source_watermark: int
    records: tuple[SnapshotRecord, ...]
    snapshot_sha256: str


def _bounded_rows(transaction, sql: str, limit: int) -> list[dict]:
    rows = transaction.query(sql, [limit + 1])["rows"]
    if len(rows) > limit:
        raise RuntimeError("PostgreSQL snapshot exceeds its configured bound")
    return rows


def _group(rows: list[dict]) -> dict[int, list[dict]]:
    grouped: dict[int, list[dict]] = {}
    for row in rows:
        item = dict(row)
        dataset_id = int(item.pop("dataset_id"))
        grouped.setdefault(dataset_id, []).append(item)
    return grouped


def _canonical(document: dict) -> tuple[str, str]:
    payload = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return payload, hashlib.sha256(payload.encode("utf-8")).hexdigest()


def snapshot_digest(records: tuple[SnapshotRecord, ...]) -> str:
    hasher = hashlib.sha256()
    for record in records:
        _, digest = _canonical(record.document)
        if digest != record.payload_sha256:
            raise RuntimeError(f"Dataset {record.record_id} payload hash mismatch")
        for value in (record.record_id, record.owner_principal, digest):
            encoded = value.encode("utf-8")
            hasher.update(len(encoded).to_bytes(8, "big"))
            hasher.update(encoded)
    return hasher.hexdigest()


def validate_snapshot(snapshot: DatasetSnapshot) -> None:
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_DATASETS:
        raise RuntimeError("Dataset snapshot metadata is invalid")
    if tuple(sorted(snapshot.records, key=lambda record: int(record.record_id))) != snapshot.records:
        raise RuntimeError("Dataset snapshot record order is invalid")
    if snapshot_digest(snapshot.records) != snapshot.snapshot_sha256:
        raise RuntimeError("Dataset snapshot identity mismatch")


def capture_dataset_snapshot() -> DatasetSnapshot:
    """Capture datasets and semantic children at one repeatable source watermark."""
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark_row = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox"
        )
        parents = _bounded_rows(transaction, """
            SELECT id, dataset_name, description, fact_table, schema_name,
                   database_name, created_at, modified_at, date_column,
                   tables_used, created_by, modified_by, visibility, 0 as favorite
            FROM datasets ORDER BY id LIMIT @param0
        """, MAX_DATASETS)
        dimensions = _group(_bounded_rows(transaction, """
            SELECT dataset_id, dimension_table, table_name, join_condition,
                   fact_key, join_key, dim_name, display_name
            FROM dataset_dimensions ORDER BY dataset_id, id LIMIT @param0
        """, MAX_COMPONENTS))
        columns = _group(_bounded_rows(transaction, """
            SELECT dataset_id, table_name, column_name, data_type,
                   is_dimension, is_metric, semantic_type
            FROM dataset_columns ORDER BY dataset_id, id LIMIT @param0
        """, MAX_COMPONENTS))
        metrics = _group(_bounded_rows(transaction, """
            SELECT dataset_id, metric_name as name, expression, metric_type, format
            FROM dataset_metrics ORDER BY dataset_id, id LIMIT @param0
        """, MAX_COMPONENTS))

    records = []
    total_bytes = 0
    for parent in parents:
        dataset_id = int(parent["id"])
        document = datasets._adapt(parent)
        document.pop("favorite", None)
        document["dimensions"] = dimensions.get(dataset_id, [])
        document["columns"] = columns.get(dataset_id, [])
        document["metrics"] = metrics.get(dataset_id, [])
        document["filters"] = []
        try:
            metadata = json.loads(parent.get("tables_used") or "{}")
            if isinstance(metadata, dict) and isinstance(metadata.get("filters"), list):
                document["filters"] = metadata["filters"]
        except (TypeError, json.JSONDecodeError):
            pass
        payload, digest = _canonical(document)
        total_bytes += len(payload.encode("utf-8"))
        if total_bytes > MAX_SNAPSHOT_BYTES:
            raise RuntimeError("PostgreSQL snapshot exceeds its byte bound")
        record_id = str(dataset_id)
        owner = str(parent["created_by"])
        records.append(SnapshotRecord(record_id, owner, document, digest))
    immutable_records = tuple(records)
    return DatasetSnapshot(
        source_watermark=int((watermark_row or {}).get("watermark") or 0),
        records=immutable_records,
        snapshot_sha256=snapshot_digest(immutable_records),
    )


def _target_matches(target: dict | None, record: SnapshotRecord) -> bool:
    if not target or target.get("document") != record.document:
        return False
    _, digest = _canonical(target["document"])
    return digest == record.payload_sha256


def apply_and_reconcile(snapshot: DatasetSnapshot) -> dict:
    """Create missing records and prove exact owner-scoped target parity."""
    validate_snapshot(snapshot)
    created = 0
    already_present = 0
    for record in snapshot.records:
        target = product_store.read("dataset", record.record_id, record.owner_principal, "Admin")
        if _target_matches(target, record):
            already_present += 1
            continue
        if target is not None:
            raise RuntimeError(f"KaveonDB dataset {record.record_id} differs from PostgreSQL snapshot")
        mutation = product_store.ProductMutation(
            "create", "dataset", record.record_id, record.document
        )
        try:
            product_store.transact([mutation], record.owner_principal, "Admin")
        except HTTPException as error:
            if error.status_code != 409:
                raise
            resolved = product_store.read(
                "dataset", record.record_id, record.owner_principal, "Admin"
            )
            if not _target_matches(resolved, record):
                raise RuntimeError(
                    f"KaveonDB dataset {record.record_id} conflict did not reconcile"
                ) from error
        created += 1

    target_generations = []
    for record in snapshot.records:
        target = product_store.read("dataset", record.record_id, record.owner_principal, "Admin")
        if not _target_matches(target, record):
            raise RuntimeError(f"KaveonDB dataset {record.record_id} failed reconciliation")
        target_generations.append(int(target["generation"]))
    return {
        "family": "datasets",
        "source_watermark": snapshot.source_watermark,
        "source_count": len(snapshot.records),
        "created": created,
        "already_present": already_present,
        "reconciled": len(snapshot.records),
        "snapshot_sha256": snapshot.snapshot_sha256,
        "max_target_generation": max(target_generations, default=0),
    }
