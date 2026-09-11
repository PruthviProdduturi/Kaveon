"""Deterministic PostgreSQL dashboard snapshot and KaveonDB reconciliation."""

import hashlib
import json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store

MAX_DASHBOARDS, MAX_DOCUMENT_BYTES, MAX_CHART_REFS = 10_000, 1024 * 1024, 1000

@dataclass(frozen=True)
class DashboardRecord:
    record_id: str; owner_principal: str; document: dict; payload_sha256: str

@dataclass(frozen=True)
class DashboardSnapshot:
    source_watermark: int; chart_snapshot_id: str; records: tuple[DashboardRecord, ...]; snapshot_sha256: str

def _canonical(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    if len(encoded) > MAX_DOCUMENT_BYTES: raise RuntimeError("dashboard document exceeds its byte bound")
    return encoded, hashlib.sha256(encoded).hexdigest()

def snapshot_digest(records, snapshot_id):
    digest = hashlib.sha256()
    for value in (snapshot_id, *(part for record in records for part in
                  (record.record_id, record.owner_principal, record.payload_sha256))):
        encoded = value.encode(); digest.update(len(encoded).to_bytes(8, "big")); digest.update(encoded)
    return digest.hexdigest()

def _json(value, label, expected):
    try: parsed = json.loads(value or json.dumps(expected))
    except (TypeError, json.JSONDecodeError) as error: raise RuntimeError(f"dashboard {label} is invalid") from error
    if not isinstance(parsed, type(expected)): raise RuntimeError(f"dashboard {label} is invalid")
    return parsed

def _text(value): return value.isoformat() if hasattr(value, "isoformat") else value

def validate_snapshot(snapshot):
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_DASHBOARDS:
        raise RuntimeError("dashboard snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda item: item.record_id)):
        raise RuntimeError("dashboard snapshot order is invalid")
    for record in snapshot.records:
        _, digest = _canonical(record.document)
        if digest != record.payload_sha256 or record.document.get("id") != record.record_id \
                or record.document.get("created_by") != record.owner_principal:
            raise RuntimeError(f"dashboard {record.record_id} is invalid")
    if snapshot_digest(snapshot.records, snapshot.chart_snapshot_id) != snapshot.snapshot_sha256:
        raise RuntimeError("dashboard snapshot identity mismatch")

def capture_snapshot():
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox") or {}
        rows = transaction.query("""SELECT id,name,description,layout,charts,filters,theme,visibility,
            is_published,is_archived,created_by,modified_by,created_at,modified_at
            FROM dashboards ORDER BY id LIMIT @param0""", [MAX_DASHBOARDS + 1])["rows"]
    if len(rows) > MAX_DASHBOARDS: raise RuntimeError("dashboard snapshot exceeds its record bound")
    records, target_snapshot = [], None
    for row in rows:
        owner, record_id = str(row.get("created_by") or ""), str(row["id"])
        if not owner: raise RuntimeError(f"dashboard {record_id} owner is missing")
        chart_ids = _json(row.get("charts"), "charts", [])
        if len(chart_ids) > MAX_CHART_REFS or len({str(value) for value in chart_ids}) != len(chart_ids):
            raise RuntimeError(f"dashboard {record_id} chart references are invalid")
        revisions = {}
        for chart_id in sorted(str(value) for value in chart_ids):
            chart = product_store.read("chart", chart_id, owner, "Admin")
            if chart is None: raise RuntimeError(f"KaveonDB chart {chart_id} is missing for dashboard")
            snapshot_id, revision = str(chart.get("snapshot_id") or ""), chart.get("revision")
            if not snapshot_id or (target_snapshot is not None and snapshot_id != target_snapshot) \
                    or type(revision) is not int or revision < 1:
                raise RuntimeError("KaveonDB chart snapshot or revision is invalid")
            target_snapshot, revisions[chart_id] = snapshot_id, revision
        visibility = row.get("visibility") or "internal"
        if visibility not in {"private", "internal", "published"}: raise RuntimeError("dashboard visibility is invalid")
        document = {"id": record_id, "name": row.get("name"), "description": row.get("description"),
            "layout": _json(row.get("layout"), "layout", []), "charts": chart_ids,
            "chart_revisions": revisions, "filters": _json(row.get("filters"), "filters", []),
            "theme": row.get("theme"), "visibility": visibility,
            "is_published": bool(row.get("is_published")), "is_archived": bool(row.get("is_archived")),
            "created_by": owner, "modified_by": str(row.get("modified_by") or owner),
            "created_at": _text(row.get("created_at")), "updated_at": _text(row.get("modified_at"))}
        _, payload_hash = _canonical(document)
        records.append(DashboardRecord(record_id, owner, document, payload_hash))
    records = tuple(sorted(records, key=lambda item: item.record_id)); target_snapshot = target_snapshot or "empty"
    return DashboardSnapshot(int(watermark.get("watermark") or 0), target_snapshot, records,
                             snapshot_digest(records, target_snapshot))

def apply_and_reconcile(snapshot):
    validate_snapshot(snapshot); created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("dashboard", record.record_id, record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document: already_present += 1; continue
        if target is not None: raise RuntimeError(f"KaveonDB dashboard {record.record_id} diverges")
        try: product_store.transact([product_store.ProductMutation("create", "dashboard", record.record_id,
             record.document)], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read("dashboard", record.record_id, record.owner_principal, "Admin")
            if error.status_code != 409 or resolved is None or resolved.get("document") != record.document: raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("dashboard", record.record_id, record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB dashboard {record.record_id} failed reconciliation")
    return {"family": "dashboards", "source_watermark": snapshot.source_watermark,
            "chart_snapshot_id": snapshot.chart_snapshot_id, "source_count": len(snapshot.records),
            "created": created, "already_present": already_present, "reconciled": len(snapshot.records),
            "snapshot_sha256": snapshot.snapshot_sha256}
