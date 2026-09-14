"""Create-only KaveonDB ADLS backup generation."""
import hashlib,json
from services import kaveondb_recovery_evidence as evidence, product_store

KINDS=("activity","chart","chat_message","chat_session","dashboard","dataset","dlm_definition","dlm_run","favorite","query_history","saved_query","source","user_recent","user_theme")
MAX_OBJECTS=10000

def product_inventory(actor="kaveon-backup"):
 records=[];snapshot=None
 for kind in KINDS:
  kind_records,current=product_store.list_records_snapshot(kind,actor,"Admin",max_records=1000)
  if snapshot and current!=snapshot:raise RuntimeError("KaveonDB product snapshot changed during backup")
  snapshot=current
  for record in kind_records:
   document=record.get("document");revision=record.get("revision")
   if not isinstance(document,dict) or not isinstance(record.get("id"),str) or type(revision) is not int or revision<1:raise RuntimeError("KaveonDB backup record is invalid")
   digest=hashlib.sha256(json.dumps(document,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
   records.append({"kind":kind,"id":record["id"],"revision":revision,"document_sha256":digest})
 identity=evidence.state_identity(records)
 return records,{**identity,"snapshot_id":snapshot or "empty"}

def create(active_prefix,backup_id,source,destination,records,state):
 if not backup_id or "/" in backup_id:raise RuntimeError("backup ID is invalid")
 source_before=sorted(source.list(active_prefix,MAX_OBJECTS),key=lambda item:item["path"])
 if len({item["path"] for item in source_before})!=len(source_before):raise RuntimeError("active transaction prefix contains duplicate objects")
 if not source_before:raise RuntimeError("active transaction prefix is empty")
 total=sum(item["size"] for item in source_before)
 if total>2*1024*1024*1024:raise RuntimeError("backup exceeds its byte bound")
 root=f"backups/{backup_id}";objects=[]
 for item in source_before:
  if not item["path"].startswith(active_prefix.rstrip("/")+"/"):raise RuntimeError("ADLS listed an object outside the active prefix")
  relative=item["path"][len(active_prefix.rstrip("/")+"/"):]
  content=source.read(item["path"],item["size"])
  if content is None or len(content)!=item["size"]:raise RuntimeError("active transaction object read is incomplete")
  digest=hashlib.sha256(content).hexdigest();etag=destination.create_if_absent_with_etag(f"{root}/active/{relative}",content)
  objects.append({"path":f"active/{relative}","etag":etag,"size":len(content),"sha256":digest})
 inventory=json.dumps(records,sort_keys=True,separators=(",",":")).encode()
 etag=destination.create_if_absent_with_etag(f"{root}/state-inventory.json",inventory)
 objects.append({"path":"state-inventory.json","etag":etag,"size":len(inventory),"sha256":hashlib.sha256(inventory).hexdigest()})
 source_after=sorted(source.list(active_prefix,MAX_OBJECTS),key=lambda item:item["path"])
 if source_before!=source_after:raise RuntimeError("active transaction prefix changed during backup")
 manifest={"schema_version":1,"backup_id":backup_id,"immutable_prefix":destination._url(root)+"/",
  "state_sha256":state["state_sha256"],"record_count":state["record_count"],"objects":objects}
 verified=evidence.validate_backup_manifest(manifest)
 manifest_bytes=json.dumps(manifest,sort_keys=True,separators=(",",":")).encode()
 destination.create_if_absent_with_etag(f"{root}/manifest.json",manifest_bytes)
 return {"backup_id":backup_id,"snapshot_id":state["snapshot_id"],"manifest_sha256":verified["manifest_sha256"],
  "state_sha256":state["state_sha256"],"record_count":state["record_count"],"object_count":len(objects),"manifest":manifest}
