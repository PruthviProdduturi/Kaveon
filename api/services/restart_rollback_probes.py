"""Bounded live restart/recovery and rollback rehearsal orchestration."""
import json,subprocess,time,math
from pathlib import Path
from services import kaveondb_recovery_evidence as recovery
from services import postgresql_free_smoke as smoke

MAX_OUTPUT=16*1024*1024;MAX_ARGS=128;MAX_ARG_BYTES=8192
FORBIDDEN_EXECUTABLES={"cmd","cmd.exe","powershell","powershell.exe","pwsh","pwsh.exe","bash","sh"}
ALLOWED_EXECUTABLES={"kubectl","kubectl.exe","python","python.exe","python3","python3.exe","kaveon","kaveon.exe"}

def command(value):
 if (not isinstance(value,dict) or set(value)!={"argv","timeout_seconds"} or not isinstance(value["argv"],list)
  or not value["argv"] or len(value["argv"])>MAX_ARGS or any(not isinstance(v,str) or not v or len(v.encode())>MAX_ARG_BYTES for v in value["argv"])
  or Path(value["argv"][0]).name.lower() in FORBIDDEN_EXECUTABLES
  or Path(value["argv"][0]).name.lower() not in ALLOWED_EXECUTABLES
  or (Path(value["argv"][0]).name.lower().startswith("python") and (len(value["argv"])<2 or value["argv"][1] in {"-c","-m"} or not value["argv"][1].endswith(".py")))
  or type(value["timeout_seconds"]) is not int or not 1<=value["timeout_seconds"]<=1800):
  raise RuntimeError("probe command is invalid or unsafe")
 return value

def run(spec,runner=subprocess.run,json_output=True):
 command(spec)
 try:r=runner(spec["argv"],shell=False,capture_output=True,timeout=spec["timeout_seconds"],check=False)
 except (OSError,subprocess.TimeoutExpired) as error:raise RuntimeError("operational probe command could not complete") from error
 out=r.stdout if isinstance(r.stdout,bytes) else str(r.stdout).encode();err=r.stderr if isinstance(r.stderr,bytes) else str(r.stderr).encode()
 if len(out)>MAX_OUTPUT or len(err)>MAX_OUTPUT:raise RuntimeError("operational probe command output is oversized")
 if r.returncode!=0:raise RuntimeError(f"operational probe command failed with exit code {r.returncode}")
 if err.strip():raise RuntimeError("operational probe command returned unexpected stderr")
 if not json_output:return None
 def unique(pairs):
  value={}
  for key,item in pairs:
   if key in value:raise ValueError("duplicate JSON key")
   value[key]=item
  return value
 try:value=json.loads(out,object_pairs_hook=unique)
 except (UnicodeDecodeError,ValueError) as error:raise RuntimeError("operational probe command returned invalid JSON") from error
 return value

def pods(value,label):
 if not isinstance(value,dict) or not isinstance(value.get("items"),list) or not value["items"]:raise RuntimeError(f"{label} pod observation is invalid")
 ids=set()
 for item in value["items"]:
  uid=(item.get("metadata") or {}).get("uid");statuses=(item.get("status") or {}).get("containerStatuses")
  if not isinstance(uid,str) or not uid or not isinstance(statuses,list) or not statuses or not all(s.get("ready") is True for s in statuses):raise RuntimeError(f"{label} pods are not ready")
  ids.add(uid)
 if len(ids)!=len(value["items"]):raise RuntimeError(f"{label} pod UIDs are duplicated")
 return ids

def inventory(value):
 if not isinstance(value,list):raise RuntimeError("state inventory probe is invalid")
 return recovery.state_identity(value)

def restart(manifest,runner=subprocess.run):
 keys={"state_before","api_pods_before","studio_pods_before","postgresql_unavailable","restart","api_pods_after","studio_pods_after","state_after","service_probes"}
 if not isinstance(manifest,dict) or set(manifest)!=keys:raise RuntimeError("restart probe manifest is invalid")
 before=inventory(run(manifest["state_before"],runner));api_before=pods(run(manifest["api_pods_before"],runner),"API");studio_before=pods(run(manifest["studio_pods_before"],runner),"Studio")
 pg=run(manifest["postgresql_unavailable"],runner)
 if pg!={"postgresql_unavailable":True}:raise RuntimeError("PostgreSQL unavailable state was not proven")
 run(manifest["restart"],runner,json_output=False)
 api_after=pods(run(manifest["api_pods_after"],runner),"API");studio_after=pods(run(manifest["studio_pods_after"],runner),"Studio")
 if api_before&api_after or studio_before&studio_after:raise RuntimeError("API or Studio pods were not replaced")
 after=inventory(run(manifest["state_after"],runner));probe=run(manifest["service_probes"],runner)
 try:smoke.verify(probe,max_age_minutes=60)
 except RuntimeError as error:raise RuntimeError("post-restart service probes failed") from error
 if before!=after:raise RuntimeError("KaveonDB state changed across restart")
 return {"postgresql_unavailable":True,"api_restarted":True,"studio_restarted":True,"probe_count":probe["check_count"],"service_state_sha256":probe["state_sha256"],
  "state_sha256_before":before["state_sha256"],"state_sha256_after":after["state_sha256"],
  "state_record_count_before":before["record_count"],"state_record_count_after":after["record_count"]}

def rollback(manifest,control,runner=subprocess.run,clock=time.monotonic):
 keys={"state_before","target_fence","rollback","source_reads","source_writes","state_after"}
 if not isinstance(manifest,dict) or set(manifest)!=keys:raise RuntimeError("rollback probe manifest is invalid")
 control=recovery.validate_rollback_control(control);before=inventory(run(manifest["state_before"],runner))
 if before["state_sha256"]!=control["expected_state_sha256"]:raise RuntimeError("rollback starting state does not match its control")
 if run(manifest["target_fence"],runner)!={"target_writes_fenced":True}:raise RuntimeError("target write fence was not proven")
 start=clock();result=run(manifest["rollback"],runner);duration=math.ceil(clock()-start)
 if duration < 0:raise RuntimeError("rollback clock moved backwards")
 if not isinstance(result,dict) or set(result)!={"cutover_revision","rollback_operation_count"} or result["cutover_revision"]!=control["cutover_revision"] or type(result["rollback_operation_count"]) is not int or not 0<=result["rollback_operation_count"]<=control["max_operations"]:raise RuntimeError("rollback command exceeded or mismatched its control")
 if duration>control["max_duration_seconds"]:raise RuntimeError("rollback command exceeded its duration bound")
 if run(manifest["source_reads"],runner)!={"source_reads_restored":True}:raise RuntimeError("source reads were not restored")
 if run(manifest["source_writes"],runner)!={"source_writes_restored":True}:raise RuntimeError("source writes were not restored")
 after=inventory(run(manifest["state_after"],runner))
 if before!=after:raise RuntimeError("KaveonDB state changed across rollback")
 return {"cutover_revision":control["cutover_revision"],"target_writes_fenced":True,"source_reads_restored":True,"source_writes_restored":True,
  "state_sha256_before":before["state_sha256"],"state_sha256_after":after["state_sha256"],"duration_seconds":duration,
  "rollback_operation_count":result["rollback_operation_count"],"rollback_operation_limit":control["max_operations"]}
