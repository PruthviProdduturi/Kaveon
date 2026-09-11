"""Deterministic non-secret activity audit snapshot and reconciliation."""
import hashlib,json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store
MAX_RECORDS=100_000;FORBIDDEN=("password","secret","token","credential","connection_string","api_key")
@dataclass(frozen=True)
class Record:record_id:str;owner_principal:str;document:dict;payload_sha256:str
@dataclass(frozen=True)
class Snapshot:source_watermark:int;records:tuple[Record,...];snapshot_sha256:str
def _safe(value):
 if isinstance(value,dict):
  for key,child in value.items():
   if any(part in str(key).lower() for part in FORBIDDEN):raise RuntimeError("activity details contain a forbidden field")
   _safe(child)
 elif isinstance(value,list):
  for child in value:_safe(child)
def canonical(value):_safe(value);return hashlib.sha256(json.dumps(value,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
def digest(records):return hashlib.sha256("".join(r.record_id+r.owner_principal+r.payload_sha256 for r in records).encode()).hexdigest()
def document(row):
 details=row.get("details")
 if isinstance(details,str):
  try:details=json.loads(details)
  except json.JSONDecodeError as error:raise RuntimeError("activity details are not structured JSON") from error
 if details is not None and not isinstance(details,dict):raise RuntimeError("activity details must be an object")
 _safe(details)
 timestamp=row.get("timestamp");timestamp=timestamp.isoformat() if hasattr(timestamp,"isoformat") else str(timestamp or "")
 return {"id":str(row["id"]),"action":row.get("action"),"object_type":row.get("object_type"),"object_id":str(row.get("object_id") or ""),"object_name":row.get("object_name"),"timestamp":timestamp,"user_email":str(row.get("user_email") or ""),"details":details}
def validate(snapshot):
 if snapshot.source_watermark<0 or len(snapshot.records)>MAX_RECORDS or snapshot.records!=tuple(sorted(snapshot.records,key=lambda r:r.record_id)):raise RuntimeError("activity snapshot metadata is invalid")
 for r in snapshot.records:
  if not r.owner_principal or r.document.get("user_email")!=r.owner_principal or r.document.get("id")!=r.record_id or not all(r.document.get(k) for k in ("action","object_type","object_id","object_name","timestamp")) or canonical(r.document)!=r.payload_sha256:raise RuntimeError("activity record is invalid")
 if digest(snapshot.records)!=snapshot.snapshot_sha256:raise RuntimeError("activity snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY");wm=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {};rows=tx.query("SELECT id,action,object_type,object_id,object_name,timestamp,user_email,details FROM activity ORDER BY id LIMIT @param0",[MAX_RECORDS+1])["rows"]
 if len(rows)>MAX_RECORDS:raise RuntimeError("activity snapshot exceeds its bound")
 records=[]
 for row in rows:
  doc=document(row);records.append(Record(doc["id"],doc["user_email"],doc,canonical(doc)))
 records=tuple(sorted(records,key=lambda r:r.record_id));snapshot=Snapshot(int(wm.get("watermark") or 0),records,digest(records));validate(snapshot);return snapshot
def apply_and_reconcile(snapshot):
 validate(snapshot);created=present=0
 for r in snapshot.records:
  target=product_store.read("activity",r.record_id,r.owner_principal,"Admin")
  if target is not None and target.get("document")==r.document:present+=1;continue
  if target is not None:raise RuntimeError("KaveonDB activity diverges")
  try:product_store.transact([product_store.ProductMutation("create","activity",r.record_id,r.document)],r.owner_principal,"Admin")
  except HTTPException as error:
   target=product_store.read("activity",r.record_id,r.owner_principal,"Admin")
   if error.status_code!=409 or target is None or target.get("document")!=r.document:raise
  created+=1
 for r in snapshot.records:
  if (product_store.read("activity",r.record_id,r.owner_principal,"Admin") or {}).get("document")!=r.document:raise RuntimeError("activity reconciliation failed")
 return {"family":"activity","source_watermark":snapshot.source_watermark,"source_count":len(snapshot.records),"created":created,"already_present":present,"reconciled":len(snapshot.records),"snapshot_sha256":snapshot.snapshot_sha256}
