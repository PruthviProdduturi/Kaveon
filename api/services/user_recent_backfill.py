"""Deterministic owner-scoped user recent snapshot and reconciliation."""
import hashlib,json
from collections import Counter
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store
MAX_RECORDS=200_000
@dataclass(frozen=True)
class Record: record_id:str;owner_principal:str;document:dict;payload_sha256:str
@dataclass(frozen=True)
class Snapshot: source_watermark:int;records:tuple[Record,...];snapshot_sha256:str
def canonical(v):
 b=json.dumps(v,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode();return hashlib.sha256(b).hexdigest()
def record_id(owner,item):return hashlib.sha256(f"{owner}\0{item}".encode()).hexdigest()
def normalize_item_id(item,item_type):
 item=str(item or "");prefix={"dataset":"dataset-","chart":"chart-","dashboard":"dashboard-"}.get(str(item_type))
 if not item or prefix is None:raise RuntimeError("user recent item reference is invalid")
 if any(item.startswith(value) for value in ("dataset-","chart-","dashboard-")):
  if not item.startswith(prefix):raise RuntimeError("user recent item prefix does not match its type")
  return item
 return prefix+item
def digest(records):return hashlib.sha256("".join(r.payload_sha256 for r in records).encode()).hexdigest()
def validate(s):
 if s.source_watermark<0 or len(s.records)>MAX_RECORDS or s.records!=tuple(sorted(s.records,key=lambda r:r.record_id)):raise RuntimeError("user recent snapshot metadata is invalid")
 counts=Counter(r.owner_principal for r in s.records)
 if any(n>20 for n in counts.values()):raise RuntimeError("user recent owner retention exceeds 20")
 for r in s.records:
  d=r.document
  if not r.owner_principal or d.get("user_email")!=r.owner_principal or d.get("type") not in {"dataset","chart","dashboard"} or r.record_id not in {record_id(r.owner_principal,str(d.get("item_id") or "")),record_id(r.owner_principal,str(d.get("item_id") or "").removeprefix(f"{d.get('type')}-"))} or canonical(d)!=r.payload_sha256:raise RuntimeError("user recent record is invalid")
 if digest(s.records)!=s.snapshot_sha256:raise RuntimeError("user recent snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
  wm=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {}
  rows=tx.query("SELECT user_email,item_id,label,href,type,created_at FROM user_recents ORDER BY user_email,created_at DESC,item_id LIMIT @param0",[MAX_RECORDS+1])["rows"]
 if len(rows)>MAX_RECORDS:raise RuntimeError("user recent snapshot exceeds its bound")
 records=[]
 for row in rows:
  owner=str(row.get("user_email") or "");item=str(row.get("item_id") or "");created=row.get("created_at");created=created.isoformat() if hasattr(created,"isoformat") else str(created or "")
  document={"user_email":owner,"item_id":normalize_item_id(item,row.get("type")),"label":row.get("label"),"href":row.get("href"),"type":row.get("type"),"created_at":created}
  records.append(Record(record_id(owner,document["item_id"]),owner,document,canonical(document)))
 records=tuple(sorted(records,key=lambda r:r.record_id));s=Snapshot(int(wm.get("watermark") or 0),records,digest(records));validate(s);return s
def apply_and_reconcile(s):
 validate(s);created=present=0
 for r in s.records:
  target=product_store.read("user_recent",r.record_id,r.owner_principal,"Admin")
  if target is not None and target.get("document")==r.document:present+=1;continue
  if target is not None:raise RuntimeError("KaveonDB user recent diverges")
  try:product_store.transact([product_store.ProductMutation("create","user_recent",r.record_id,r.document)],r.owner_principal,"Admin")
  except HTTPException as error:
   target=product_store.read("user_recent",r.record_id,r.owner_principal,"Admin")
   if error.status_code!=409 or target is None or target.get("document")!=r.document:raise
  created+=1
 for r in s.records:
  if (product_store.read("user_recent",r.record_id,r.owner_principal,"Admin") or {}).get("document")!=r.document:raise RuntimeError("user recent reconciliation failed")
 return {"family":"user_recents","source_watermark":s.source_watermark,"source_count":len(s.records),"created":created,"already_present":present,"reconciled":len(s.records),"snapshot_sha256":s.snapshot_sha256}
