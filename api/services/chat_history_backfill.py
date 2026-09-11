"""Deterministic owner-isolated chat snapshot and exact reconciliation."""
import hashlib,json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store
MAX_SESSIONS=25_000;MAX_MESSAGES=250_000;MAX_DOCUMENT_BYTES=1024*1024
@dataclass(frozen=True)
class Record:kind:str;record_id:str;owner_principal:str;document:dict;payload_sha256:str
@dataclass(frozen=True)
class Snapshot:source_watermark:int;records:tuple[Record,...];snapshot_sha256:str
def canonical(v):
 b=json.dumps(v,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
 if len(b)>MAX_DOCUMENT_BYTES:raise RuntimeError("chat document exceeds its bound")
 return hashlib.sha256(b).hexdigest()
def digest(records):return hashlib.sha256("".join(r.kind+r.record_id+r.owner_principal+r.payload_sha256 for r in records).encode()).hexdigest()
def text(v):return v.isoformat() if hasattr(v,"isoformat") else v
def session_document(r):return {"id":str(r["id"]),"user_email":str(r.get("user_email") or ""),"title":r.get("title"),"created_at":text(r.get("created_at")),"updated_at":text(r.get("updated_at"))}
def message_document(r):
 data=r.get("data")
 if isinstance(data,str):
  try:data=json.loads(data)
  except json.JSONDecodeError as error:raise RuntimeError("chat message data is invalid") from error
 return {"id":str(r["id"]),"session_id":str(r["session_id"]),"user_email":str(r.get("user_email") or ""),"role":r.get("role"),"content":r.get("content"),"sql_query":r.get("sql_query"),"chart_type":r.get("chart_type"),"data":data,"route":r.get("route"),"created_at":text(r.get("created_at"))}
def order(r):return (0 if r.kind=="chat_session" else 1,r.record_id)
def validate(s):
 if s.source_watermark<0 or len(s.records)>MAX_SESSIONS+MAX_MESSAGES or s.records!=tuple(sorted(s.records,key=order)):raise RuntimeError("chat snapshot metadata is invalid")
 sessions={r.record_id for r in s.records if r.kind=="chat_session"}
 for r in s.records:
  if r.kind not in {"chat_session","chat_message"} or not r.owner_principal or r.document.get("user_email")!=r.owner_principal or r.document.get("id")!=r.record_id or canonical(r.document)!=r.payload_sha256:raise RuntimeError("chat record is invalid")
  if r.kind=="chat_message" and r.document.get("session_id") not in sessions:raise RuntimeError("chat message session is missing")
 if digest(s.records)!=s.snapshot_sha256:raise RuntimeError("chat snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY");wm=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {};sessions=tx.query("SELECT id,user_email,title,created_at,updated_at FROM chat_sessions ORDER BY id LIMIT @param0",[MAX_SESSIONS+1])["rows"];messages=tx.query("SELECT m.id,m.session_id,s.user_email,m.role,m.content,m.sql_query,m.chart_type,m.data,m.route,m.created_at FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id ORDER BY m.id LIMIT @param0",[MAX_MESSAGES+1])["rows"]
 if len(sessions)>MAX_SESSIONS or len(messages)>MAX_MESSAGES:raise RuntimeError("chat snapshot exceeds its bound")
 records=[]
 for kind,rows,builder in [("chat_session",sessions,session_document),("chat_message",messages,message_document)]:
  for row in rows:
   doc=builder(row);records.append(Record(kind,doc["id"],doc["user_email"],doc,canonical(doc)))
 records=tuple(sorted(records,key=order));snapshot=Snapshot(int(wm.get("watermark") or 0),records,digest(records));validate(snapshot);return snapshot
def apply_and_reconcile(s):
 validate(s);created=present=0
 for r in s.records:
  if apply_record(r):created+=1
  else:present+=1
 for r in s.records:
  if (product_store.read(r.kind,r.record_id,r.owner_principal,"Admin") or {}).get("document")!=r.document:raise RuntimeError("chat reconciliation failed")
 return {"family":"chat_history","source_watermark":s.source_watermark,"source_count":len(s.records),"created":created,"already_present":present,"reconciled":len(s.records),"snapshot_sha256":s.snapshot_sha256}
def apply_record(r):
 if r.kind not in {"chat_session","chat_message"} or not r.owner_principal or canonical(r.document)!=r.payload_sha256:raise RuntimeError("chat record is invalid")
 target=product_store.read(r.kind,r.record_id,r.owner_principal,"Admin")
 if target is not None and target.get("document")==r.document:return False
 if target is not None:raise RuntimeError("KaveonDB chat record diverges")
 try:product_store.transact([product_store.ProductMutation("create",r.kind,r.record_id,r.document)],r.owner_principal,"Admin")
 except HTTPException as error:
  target=product_store.read(r.kind,r.record_id,r.owner_principal,"Admin")
  if error.status_code!=409 or target is None or target.get("document")!=r.document:raise
 return True
