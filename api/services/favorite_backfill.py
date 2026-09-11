"""Deterministic PostgreSQL favorite snapshot and exact target reconciliation."""
import hashlib,json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import favorites,product_store
MAX_FAVORITES=100_000
@dataclass(frozen=True)
class FavoriteRecord: record_id:str; owner_principal:str; document:dict; payload_sha256:str
@dataclass(frozen=True)
class FavoriteSnapshot: source_watermark:int; target_snapshot_id:str; records:tuple[FavoriteRecord,...]; snapshot_sha256:str
def _canonical(v):
 b=json.dumps(v,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode(); return b,hashlib.sha256(b).hexdigest()
def snapshot_digest(records,sid):
 h=hashlib.sha256()
 for v in (sid,*(x for r in records for x in (r.record_id,r.owner_principal,r.payload_sha256))):
  b=v.encode();h.update(len(b).to_bytes(8,"big"));h.update(b)
 return h.hexdigest()
def validate_snapshot(s):
 if s.source_watermark<0 or len(s.records)>MAX_FAVORITES: raise RuntimeError("favorite snapshot metadata is invalid")
 if s.records!=tuple(sorted(s.records,key=lambda r:r.record_id)): raise RuntimeError("favorite snapshot order is invalid")
 for r in s.records:
  if _canonical(r.document)[1]!=r.payload_sha256 or r.document.get("user_email")!=r.owner_principal or favorites._record_id(r.owner_principal,r.document.get("object_type"),r.document.get("object_id"))!=r.record_id: raise RuntimeError(f"favorite {r.record_id} is invalid")
 if snapshot_digest(s.records,s.target_snapshot_id)!=s.snapshot_sha256: raise RuntimeError("favorite snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
  watermark=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {}
  rows=tx.query("SELECT id,user_email,object_id,object_type,object_name FROM favorites ORDER BY user_email,object_type,object_id LIMIT @param0",[MAX_FAVORITES+1])["rows"]
 if len(rows)>MAX_FAVORITES: raise RuntimeError("favorite snapshot exceeds its record bound")
 records=[]; sid=None
 for row in rows:
  kind=str(row.get("object_type") or "")
  owner=str(row.get("user_email") or "");oid=str(row.get("object_id") or "")
  if kind=="data_source": kind,oid="source",f"data:{oid}"
  if kind not in favorites.MIGRATABLE_TYPES: raise RuntimeError("favorite object type is unsupported")
  target=product_store.read(kind,oid,owner,"Admin")
  if target is None: raise RuntimeError(f"favorite target {kind}/{oid} is missing")
  current=str(target.get("snapshot_id") or "")
  if not current or (sid is not None and sid!=current): raise RuntimeError("favorite target snapshot is invalid")
  sid=current;document={"user_email":owner,"object_type":kind,"object_id":oid,"object_name":row.get("object_name")};rid=favorites._record_id(owner,kind,oid)
  records.append(FavoriteRecord(rid,owner,document,_canonical(document)[1]))
 records=tuple(sorted(records,key=lambda r:r.record_id));sid=sid or "empty"
 result=FavoriteSnapshot(int(watermark.get("watermark") or 0),sid,records,snapshot_digest(records,sid));validate_snapshot(result);return result
def apply_and_reconcile(s):
 validate_snapshot(s);created=present=0
 for r in s.records:
  target=product_store.read("favorite",r.record_id,r.owner_principal,"Admin")
  if target is not None and target.get("document")==r.document:present+=1;continue
  if target is not None:raise RuntimeError(f"KaveonDB favorite {r.record_id} diverges")
  try:product_store.transact([product_store.ProductMutation("create","favorite",r.record_id,r.document)],r.owner_principal,"Admin")
  except HTTPException as e:
   resolved=product_store.read("favorite",r.record_id,r.owner_principal,"Admin")
   if e.status_code!=409 or resolved is None or resolved.get("document")!=r.document:raise
  created+=1
 for r in s.records:
  target=product_store.read("favorite",r.record_id,r.owner_principal,"Admin")
  if target is None or target.get("document")!=r.document:raise RuntimeError(f"KaveonDB favorite {r.record_id} failed reconciliation")
 return {"family":"favorites","source_watermark":s.source_watermark,"target_snapshot_id":s.target_snapshot_id,"source_count":len(s.records),"created":created,"already_present":present,"reconciled":len(s.records),"snapshot_sha256":s.snapshot_sha256}
