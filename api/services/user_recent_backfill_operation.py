"""Tamper-evident, resumable user-recent backfill; dry-run by default."""
import hashlib,json,os,tempfile
from pathlib import Path
from services import user_recent_backfill as backfill
VERSION=1;MAX_CHECKPOINT_BYTES=16*1024*1024
def _body(snapshot,index,complete):return {"version":VERSION,"family":"user_recents","source_watermark":snapshot.source_watermark,"snapshot_sha256":snapshot.snapshot_sha256,"next_index":index,"complete":complete,"records":[r.__dict__ for r in snapshot.records]}
def save(path,snapshot,index,complete=False):
 backfill.validate(snapshot)
 if not 0<=index<=len(snapshot.records) or (complete and index!=len(snapshot.records)):raise RuntimeError("user recent checkpoint position is invalid")
 body=_body(snapshot,index,complete);body["checkpoint_sha256"]=hashlib.sha256(json.dumps(body,sort_keys=True,separators=(",",":")).encode()).hexdigest();encoded=json.dumps(body,sort_keys=True,separators=(",",":")).encode()
 if len(encoded)>MAX_CHECKPOINT_BYTES:raise RuntimeError("user recent checkpoint exceeds its bound")
 path=path.resolve();path.parent.mkdir(parents=True,exist_ok=True);temporary=None
 try:
  with tempfile.NamedTemporaryFile(mode="wb",dir=path.parent,prefix=path.name+".",delete=False) as handle:temporary=Path(handle.name);os.chmod(temporary,0o600);handle.write(encoded);handle.flush();os.fsync(handle.fileno())
  os.replace(temporary,path)
 finally:
  if temporary and temporary.exists():temporary.unlink()
def load(path):
 if path.stat().st_size>MAX_CHECKPOINT_BYTES:raise RuntimeError("user recent checkpoint exceeds its bound")
 try:
  raw=json.loads(path.read_text());claimed=raw.pop("checkpoint_sha256")
  if claimed!=hashlib.sha256(json.dumps(raw,sort_keys=True,separators=(",",":")).encode()).hexdigest() or raw["version"]!=VERSION or raw["family"]!="user_recents":raise RuntimeError("user recent checkpoint identity is invalid")
  records=tuple(backfill.Record(**r) for r in raw["records"]);snapshot=backfill.Snapshot(int(raw["source_watermark"]),records,str(raw["snapshot_sha256"]));backfill.validate(snapshot);index=int(raw["next_index"]);complete=bool(raw["complete"])
  if not 0<=index<=len(records) or (complete and index!=len(records)):raise RuntimeError("user recent checkpoint position is invalid")
  return snapshot,index,complete
 except (KeyError,TypeError,ValueError,json.JSONDecodeError) as error:raise RuntimeError("user recent checkpoint is invalid") from error
def run(path,*,apply,resume):
 if apply and os.getenv("KAVEON_USER_RECENT_MIGRATION_ENABLED")!="true":raise RuntimeError("apply requires KAVEON_USER_RECENT_MIGRATION_ENABLED=true")
 if resume:snapshot,index,complete=load(path)
 else:
  if path.exists():raise RuntimeError("checkpoint exists; use resume")
  snapshot,index,complete=backfill.capture_snapshot(),0,False;save(path,snapshot,index)
 if not apply:return {"mode":"dry-run","next_index":index,"complete":complete,"source_count":len(snapshot.records),"snapshot_sha256":snapshot.snapshot_sha256}
 for position in range(index,len(snapshot.records)):
  record=snapshot.records[position];single=backfill.Snapshot(snapshot.source_watermark,(record,),backfill.digest((record,)));backfill.apply_and_reconcile(single);save(path,snapshot,position+1)
 report=backfill.apply_and_reconcile(snapshot);save(path,snapshot,len(snapshot.records),True);return {**report,"mode":"apply","checkpoint_complete":True}
