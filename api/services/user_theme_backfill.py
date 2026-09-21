"""Deterministic PostgreSQL user-theme snapshot and exact target reconciliation."""

import hashlib, json, re
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store

MAX_THEMES = 100_000

@dataclass(frozen=True)
class ThemeRecord:
    record_id: str; document: dict; payload_sha256: str

@dataclass(frozen=True)
class ThemeSnapshot:
    source_watermark: int; records: tuple[ThemeRecord, ...]; snapshot_sha256: str

def _canonical(value):
    encoded=json.dumps(value,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
    return encoded,hashlib.sha256(encoded).hexdigest()

def snapshot_digest(records):
    digest=hashlib.sha256()
    for record in records:
        for value in (record.record_id,record.payload_sha256):
            encoded=value.encode(); digest.update(len(encoded).to_bytes(8,"big")); digest.update(encoded)
    return digest.hexdigest()

def validate_snapshot(snapshot):
    if snapshot.source_watermark<0 or len(snapshot.records)>MAX_THEMES: raise RuntimeError("user theme snapshot metadata is invalid")
    if snapshot.records!=tuple(sorted(snapshot.records,key=lambda item:item.record_id)): raise RuntimeError("user theme snapshot order is invalid")
    for record in snapshot.records:
        _,digest=_canonical(record.document)
        if digest!=record.payload_sha256 or record.document.get("user_email")!=record.record_id \
                or re.fullmatch(r"#[0-9a-f]{6}",record.document.get("theme_color", "")) is None:
            raise RuntimeError(f"user theme {record.record_id} is invalid")
    if snapshot_digest(snapshot.records)!=snapshot.snapshot_sha256: raise RuntimeError("user theme snapshot identity mismatch")

def capture_snapshot():
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark=transaction.query_one("SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox") or {}
        rows=transaction.query("SELECT user_email, theme_color FROM user_themes ORDER BY user_email LIMIT @param0",[MAX_THEMES+1])["rows"]
    if len(rows)>MAX_THEMES: raise RuntimeError("user theme snapshot exceeds its record bound")
    records=[]
    for row in rows:
        owner=str(row.get("user_email") or ""); color=str(row.get("theme_color") or "").lower()
        document={"user_email":owner,"theme_color":color}; _,digest=_canonical(document)
        records.append(ThemeRecord(owner,document,digest))
    records=tuple(records); snapshot=ThemeSnapshot(int(watermark.get("watermark") or 0),records,snapshot_digest(records))
    validate_snapshot(snapshot); return snapshot

def apply_and_reconcile(snapshot):
    validate_snapshot(snapshot); created=already_present=repaired=0
    for record in snapshot.records:
        target=product_store.migration_read("user_theme",record.record_id,record.record_id,"Admin")
        if target is not None and target.get("document")==record.document: already_present+=1; continue
        revision=target.get("revision") if target is not None else None
        if target is not None and (type(revision) is not int or revision<1): raise RuntimeError(f"KaveonDB user theme {record.record_id} has an invalid revision")
        try: product_store.migration_transact([product_store.ProductMutation("update" if target is not None else "create","user_theme",record.record_id,record.document,revision)],record.record_id,"Admin")
        except HTTPException as error:
            resolved=product_store.migration_read("user_theme",record.record_id,record.record_id,"Admin")
            if error.status_code!=409 or resolved is None or resolved.get("document")!=record.document: raise
        if target is None: created+=1
        else: repaired+=1
    for record in snapshot.records:
        target=product_store.migration_read("user_theme",record.record_id,record.record_id,"Admin")
        if target is None or target.get("document")!=record.document: raise RuntimeError(f"KaveonDB user theme {record.record_id} failed reconciliation")
    return {"family":"user_themes","source_watermark":snapshot.source_watermark,"source_count":len(snapshot.records),
            "created":created,"already_present":already_present,"repaired":repaired,"reconciled":len(snapshot.records),"snapshot_sha256":snapshot.snapshot_sha256}
