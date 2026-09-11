"""Tamper-evident checkpoint/resume for query-history migration."""
import hashlib,json,os,tempfile
from pathlib import Path
from services import query_history_backfill as backfill
VERSION=1;MAX_CHECKPOINT_BYTES=64*1024*1024
def _body(s,index,complete):return {"version":VERSION,"family":"query_history","source_watermark":s.source_watermark,"snapshot_sha256":s.snapshot_sha256,"next_index":index,"complete":complete,"records":[r.__dict__ for r in s.records]}
def save(path,s,index,complete=False):
 backfill.validate(s)
 if not 0<=index<=len(s.records) or (complete and index!=len(s.records)):raise RuntimeError("query history checkpoint position is invalid")
 body=_body(s,index,complete);body["checkpoint_sha256"]=hashlib.sha256(json.dumps(body,sort_keys=True,separators=(",",":")).encode()).hexdigest();encoded=json.dumps(body,sort_keys=True,separators=(",",":")).encode()
 if len(encoded)>MAX_CHECKPOINT_BYTES:raise RuntimeError("query history checkpoint exceeds its bound")
 path=path.resolve();path.parent.mkdir(parents=True,exist_ok=True);temporary=None
 try:
  with tempfile.NamedTemporaryFile(mode="wb",dir=path.parent,prefix=path.name+".",delete=False) as handle:temporary=Path(handle.name);os.chmod(temporary,0o600);handle.write(encoded);handle.flush();os.fsync(handle.fileno())
  os.replace(temporary,path)
 finally:
  if temporary and temporary.exists():temporary.unlink()
def load(path):
 if path.stat().st_size>MAX_CHECKPOINT_BYTES:raise RuntimeError("query history checkpoint exceeds its bound")
 try:
  raw=json.loads(path.read_text());claimed=raw.pop("checkpoint_sha256")
  if claimed!=hashlib.sha256(json.dumps(raw,sort_keys=True,separators=(",",":")).encode()).hexdigest() or raw["version"]!=VERSION or raw["family"]!="query_history":raise RuntimeError("query history checkpoint identity is invalid")
  records=tuple(backfill.Record(**r) for r in raw["records"]);s=backfill.Snapshot(int(raw["source_watermark"]),records,str(raw["snapshot_sha256"]));backfill.validate(s);index=int(raw["next_index"]);complete=bool(raw["complete"])
  if not 0<=index<=len(records) or (complete and index!=len(records)):raise RuntimeError("query history checkpoint position is invalid")
  return s,index,complete
 except (KeyError,TypeError,ValueError,json.JSONDecodeError) as error:raise RuntimeError("query history checkpoint is invalid") from error
def run(path,*,apply,resume):
 if apply and os.getenv("KAVEON_QUERY_HISTORY_MIGRATION_ENABLED")!="true":raise RuntimeError("apply requires KAVEON_QUERY_HISTORY_MIGRATION_ENABLED=true")
 if resume:
  if not path.exists():raise RuntimeError("resume requires a checkpoint")
  s,index,complete=load(path)
 else:
  if path.exists():raise RuntimeError("checkpoint exists; use resume")
  s,index,complete=backfill.capture_snapshot(),0,False;save(path,s,index)
 if not apply:return {"mode":"dry-run","next_index":index,"complete":complete,"source_count":len(s.records),"snapshot_sha256":s.snapshot_sha256}
 for position in range(index,len(s.records)):
  record=s.records[position];single=backfill.Snapshot(s.source_watermark,(record,),backfill.digest((record,)));backfill.apply_and_reconcile(single);save(path,s,position+1)
 report=backfill.apply_and_reconcile(s);save(path,s,len(s.records),True);return {**report,"mode":"apply","checkpoint_complete":True}
