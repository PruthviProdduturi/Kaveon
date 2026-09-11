"""Bounded deterministic query-history snapshot and reconciliation."""
import hashlib,json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store
MAX_RECORDS=100_000;MAX_PER_OWNER=1_000;MAX_DOCUMENT_BYTES=1024*1024
@dataclass(frozen=True)
class Record:record_id:str;owner_principal:str;document:dict;payload_sha256:str
@dataclass(frozen=True)
class Snapshot:source_watermark:int;records:tuple[Record,...];snapshot_sha256:str
def canonical(value):
 encoded=json.dumps(value,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
 if len(encoded)>MAX_DOCUMENT_BYTES:raise RuntimeError("query history document exceeds its bound")
 return hashlib.sha256(encoded).hexdigest()
def digest(records):return hashlib.sha256("".join(r.record_id+r.owner_principal+r.payload_sha256 for r in records).encode()).hexdigest()
def document(row):
 def text(value):return value.isoformat() if hasattr(value,"isoformat") else value
 value={key:text(row.get(key)) for key in ("id","sql_text","database_name","executed_at","execution_time","row_count","status","error_message","user_email","trigger_source","dataset_id","tables_used")};value["id"]=str(value["id"]);value["user_email"]=str(value["user_email"] or "");value["dataset_id"]=str(value["dataset_id"]) if value["dataset_id"] is not None else None;return value
def validate(snapshot):
 if snapshot.source_watermark<0 or len(snapshot.records)>MAX_RECORDS or snapshot.records!=tuple(sorted(snapshot.records,key=lambda r:r.record_id)):raise RuntimeError("query history snapshot metadata is invalid")
 counts={}
 for r in snapshot.records:
  counts[r.owner_principal]=counts.get(r.owner_principal,0)+1
  if not r.owner_principal or r.document.get("user_email")!=r.owner_principal or str(r.document.get("id"))!=r.record_id or not r.document.get("sql_text") or not r.document.get("executed_at") or canonical(r.document)!=r.payload_sha256:raise RuntimeError("query history record is invalid")
 if any(count>MAX_PER_OWNER for count in counts.values()):raise RuntimeError("query history owner retention exceeds its bound")
 if digest(snapshot.records)!=snapshot.snapshot_sha256:raise RuntimeError("query history snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY");wm=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {}
  rows=tx.query("SELECT id,sql_text,database_name,executed_at,execution_time,row_count,status,error_message,user_email,trigger_source,dataset_id,tables_used FROM (SELECT id,sql_text,database_name,executed_at,execution_time,row_count,status,error_message,user_email,trigger_source,dataset_id,tables_used,ROW_NUMBER() OVER (PARTITION BY user_email ORDER BY executed_at DESC,id DESC) AS retention_rank FROM query_history) retained WHERE retention_rank<=@param0 ORDER BY id LIMIT @param1",[MAX_PER_OWNER,MAX_RECORDS+1])["rows"]
 if len(rows)>MAX_RECORDS:raise RuntimeError("query history snapshot exceeds its bound")
 records=tuple(sorted((Record(str(r["id"]),str(r.get("user_email") or ""),document(r),canonical(document(r))) for r in rows),key=lambda r:r.record_id));snapshot=Snapshot(int(wm.get("watermark") or 0),records,digest(records));validate(snapshot);return snapshot
def apply_and_reconcile(snapshot):
 validate(snapshot);created=present=0
 for r in snapshot.records:
  target=product_store.read("query_history",r.record_id,r.owner_principal,"Admin")
  if target is not None and target.get("document")==r.document:present+=1;continue
  if target is not None:raise RuntimeError("KaveonDB query history diverges")
  try:product_store.transact([product_store.ProductMutation("create","query_history",r.record_id,r.document)],r.owner_principal,"Admin")
  except HTTPException as error:
   target=product_store.read("query_history",r.record_id,r.owner_principal,"Admin")
   if error.status_code!=409 or target is None or target.get("document")!=r.document:raise
  created+=1
 for r in snapshot.records:
  if (product_store.read("query_history",r.record_id,r.owner_principal,"Admin") or {}).get("document")!=r.document:raise RuntimeError("query history reconciliation failed")
 return {"family":"query_history","source_watermark":snapshot.source_watermark,"source_count":len(snapshot.records),"created":created,"already_present":present,"reconciled":len(snapshot.records),"snapshot_sha256":snapshot.snapshot_sha256}
