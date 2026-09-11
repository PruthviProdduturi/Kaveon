"""Credential-free public source snapshot; secrets remain PostgreSQL authority."""
import hashlib,json
from dataclasses import dataclass
from fastapi import HTTPException
import database.metadata as db
from services import product_store
MAX_SOURCES=10_000
FORBIDDEN=("password","secret","token","credential","connection_string","cipher")
@dataclass(frozen=True)
class SourceRecord: record_id:str; owner_principal:str; document:dict; payload_sha256:str
@dataclass(frozen=True)
class SourceSnapshot: source_watermark:int; records:tuple[SourceRecord,...]; snapshot_sha256:str
def _canonical(v):
 b=json.dumps(v,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode();return b,hashlib.sha256(b).hexdigest()
def _safe(v):
 if isinstance(v,dict):
  for k,x in v.items():
   if k!="secret_ref" and any(p in str(k).lower() for p in FORBIDDEN):raise RuntimeError("source document contains forbidden secret-shaped field")
   _safe(x)
 elif isinstance(v,list):
  for x in v:_safe(x)
def digest(records):
 h=hashlib.sha256()
 for r in records:
  for v in (r.record_id,r.owner_principal,r.payload_sha256):b=v.encode();h.update(len(b).to_bytes(8,"big"));h.update(b)
 return h.hexdigest()
def _catalog_secret_ref(row):
 ref=row.get("credential_ref")
 if not ref:return f"identity:{row.get('credential_kind') or 'managed_identity'}"
 ref=str(ref)
 if not (ref.startswith("https://") and ".vault.azure.net/" in ref):
  raise RuntimeError("catalog credential reference is not a Key Vault reference")
 return ref
def validate_snapshot(s):
 if s.source_watermark<0 or len(s.records)>MAX_SOURCES:raise RuntimeError("source snapshot metadata is invalid")
 if s.records!=tuple(sorted(s.records,key=lambda r:r.record_id)):raise RuntimeError("source snapshot order is invalid")
 identities={}
 for r in s.records:
  _safe(r.document)
  if _canonical(r.document)[1]!=r.payload_sha256 or r.document.get("source_id")!=r.record_id or not r.owner_principal:raise RuntimeError(f"source {r.record_id} is invalid")
  identity=r.document.get("catalog_identity")
  if identity:
   identities.setdefault(identity,[]).append((r.document.get("source_kind"),r.record_id))
 if any(len(items)>2 or len({kind for kind,_ in items})!=len(items) for items in identities.values()):
  raise RuntimeError("source catalog identity is ambiguous")
 if digest(s.records)!=s.snapshot_sha256:raise RuntimeError("source snapshot identity mismatch")
def capture_snapshot():
 with db.transaction() as tx:
  tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
  wm=tx.query_one("SELECT COALESCE(MAX(source_sequence),0) AS watermark FROM product_migration_outbox") or {}
  cats=tx.query("SELECT id,name,engine_catalog,storage_type,data_format,credential_kind,credential_ref,adapter_type,lifecycle,description,created_by FROM catalog_sources ORDER BY id LIMIT @param0",[MAX_SOURCES+1])["rows"]
  data=tx.query("SELECT id,name,type,database_name,region,description,created_by,is_active FROM data_sources ORDER BY id LIMIT @param0",[MAX_SOURCES+1])["rows"]
 if len(cats)+len(data)>MAX_SOURCES:raise RuntimeError("source snapshot exceeds its record bound")
 records=[]
 for row in cats:
  rid=f"catalog:{row['id']}";owner=str(row.get("created_by") or "")
  ref=_catalog_secret_ref(row)
  doc={"source_kind":"catalog","source_id":rid,"name":row.get("name"),"catalog_identity":row.get("engine_catalog"),"source_type":row.get("storage_type"),"database_name":None,"region":None,"description":row.get("description"),"is_active":row.get("lifecycle")=="active","lifecycle":row.get("lifecycle"),"secret_ref":ref}
  records.append(SourceRecord(rid,owner,doc,_canonical(doc)[1]))
 for row in data:
  rid=f"data:{row['id']}";owner=str(row.get("created_by") or "")
  doc={"source_kind":"data","source_id":rid,"name":row.get("name"),"catalog_identity":row.get("database_name"),"source_type":row.get("type"),"database_name":row.get("database_name"),"region":row.get("region"),"description":row.get("description"),"is_active":bool(row.get("is_active")),"lifecycle":"active" if row.get("is_active") else "suspended","secret_ref":f"key-managed:data_sources/{row['id']}"}
  records.append(SourceRecord(rid,owner,doc,_canonical(doc)[1]))
 records=tuple(sorted(records,key=lambda r:r.record_id));s=SourceSnapshot(int(wm.get("watermark") or 0),records,digest(records));validate_snapshot(s);return s
def apply_and_reconcile(s):
 validate_snapshot(s);created=present=0
 for r in s.records:
  target=product_store.read("source",r.record_id,r.owner_principal,"Admin")
  if target is not None and target.get("document")==r.document:present+=1;continue
  if target is not None:raise RuntimeError(f"KaveonDB source {r.record_id} diverges")
  try:product_store.transact([product_store.ProductMutation("create","source",r.record_id,r.document)],r.owner_principal,"Admin")
  except HTTPException as e:
   resolved=product_store.read("source",r.record_id,r.owner_principal,"Admin")
   if e.status_code!=409 or resolved is None or resolved.get("document")!=r.document:raise
  created+=1
 for r in s.records:
  target=product_store.read("source",r.record_id,r.owner_principal,"Admin")
  if target is None or target.get("document")!=r.document:raise RuntimeError("source reconciliation failed")
 return {"family":"sources","source_watermark":s.source_watermark,"source_count":len(s.records),"created":created,"already_present":present,"reconciled":len(s.records),"snapshot_sha256":s.snapshot_sha256}
