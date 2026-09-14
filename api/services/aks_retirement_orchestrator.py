"""Fail-closed, resumable execution of the AKS PostgreSQL retirement rehearsal."""
import hashlib,json,os,subprocess,tempfile
from pathlib import Path
import yaml

STEPS=("snapshot_inventory","helm_migration","dlm_migration","reconcile_16","shadow_parity","fence_drain","backup","restore","restart_recovery","rollback","collect_operational","final_audit")
ALLOWED={"helm","helm.exe","kubectl","kubectl.exe","python","python.exe","python3","python3.exe"}
FORBIDDEN_KEYS={"password","accesstoken","token","connectionstring","clientsecret","accountkey","sas"}
MAX_PLAN=256*1024

def canonical(value):return json.dumps(value,sort_keys=True,separators=(",",":")).encode()
def validate_overlay(path):
 if not path.is_file() or path.stat().st_size>256*1024:raise RuntimeError("retirement values overlay is missing or oversized")
 try:value=yaml.safe_load(path.read_text(encoding="utf-8"))
 except (OSError,yaml.YAMLError) as error:raise RuntimeError("retirement values overlay is invalid") from error
 def visit(item):
  if isinstance(item,dict):
   for key,child in item.items():
    normalized=str(key).replace("_","").replace("-","").lower()
    if normalized in FORBIDDEN_KEYS:raise RuntimeError(f"retirement values overlay contains secret field: {key}")
    visit(child)
  elif isinstance(item,list):
   for child in item:visit(child)
  elif isinstance(item,str) and any(marker in item.lower() for marker in ("accountkey=","sharedaccesssignature=","?sig=")):
   raise RuntimeError("retirement values overlay contains secret material")
 visit(value);return value

def load_plan(path,overlay):
 if not path.is_file() or path.stat().st_size>MAX_PLAN:raise RuntimeError("retirement plan is missing or oversized")
 try:value=json.loads(path.read_text(encoding="utf-8"))
 except (OSError,ValueError) as error:raise RuntimeError("retirement plan is invalid") from error
 if not isinstance(value,dict) or set(value)!={"schema_version","run_id","steps"} or value["schema_version"]!=1 or not isinstance(value["run_id"],str) or not value["run_id"]:raise RuntimeError("retirement plan schema is invalid")
 steps=value["steps"]
 if not isinstance(steps,list) or [s.get("id") for s in steps if isinstance(s,dict)]!=list(STEPS):raise RuntimeError("retirement plan step order is invalid")
 for step in steps:
  if set(step)!={"id","commands"} or not isinstance(step["commands"],list) or not step["commands"]:raise RuntimeError(f"retirement commands are invalid for {step['id']}")
  for entry in step["commands"]:
   if set(entry)!={"argv","timeout_seconds"} or not isinstance(entry["argv"],list) or not entry["argv"] or Path(entry["argv"][0]).name.lower() not in ALLOWED or any(not isinstance(a,str) or not a or len(a)>8192 for a in entry["argv"]) or type(entry["timeout_seconds"]) is not int or not 1<=entry["timeout_seconds"]<=3600:raise RuntimeError(f"retirement command is invalid for {step['id']}")
   if Path(entry["argv"][0]).name.lower().startswith("python") and (len(entry["argv"])<2 or not entry["argv"][1].endswith(".py")):raise RuntimeError(f"retirement Python command is unsafe for {step['id']}")
 helm=steps[1]["commands"][0]["argv"]
 if "-f" not in helm:raise RuntimeError("Helm migration step does not consume the reviewed values overlay")
 index=helm.index("-f")
 if index+1>=len(helm) or Path(helm[index+1]).resolve()!=overlay.resolve():raise RuntimeError("Helm migration step does not consume the reviewed values overlay")
 required={"shadow_parity":"probe-kaveondb-cutover.py","backup":"create-kaveondb-adls-backup.py","restore":"rehearse-kaveondb-adls-restore.py","restart_recovery":"probe-kaveondb-recovery.py","rollback":"probe-kaveondb-recovery.py","final_audit":"run-postgresql-retirement-evidence.py"}
 for name,script in required.items():
  commands=steps[STEPS.index(name)]["commands"]
  if not any(any(Path(arg).name==script for arg in entry["argv"]) for entry in commands):raise RuntimeError(f"retirement {name} step does not use {script}")
 return value

def run(plan,checkpoint,runner=subprocess.run):
 digest=hashlib.sha256(canonical(plan)).hexdigest();completed=[]
 helm=plan["steps"][1]["commands"][0]["argv"]
 if "-f" not in helm or helm.index("-f")+1>=len(helm):raise RuntimeError("retirement plan lost its values overlay")
 overlay=Path(helm[helm.index("-f")+1])
 if not overlay.is_file() or overlay.stat().st_size>256*1024:raise RuntimeError("retirement values overlay is missing or oversized")
 overlay_sha256=hashlib.sha256(overlay.read_bytes()).hexdigest()
 if checkpoint.exists():
  try:state=json.loads(checkpoint.read_text(encoding="utf-8"))
  except (OSError,ValueError) as error:raise RuntimeError("retirement checkpoint is invalid") from error
  if set(state)!={"schema_version","run_id","plan_sha256","overlay_sha256","completed"} or state["schema_version"]!=1 or state["run_id"]!=plan["run_id"] or state["plan_sha256"]!=digest or state["overlay_sha256"]!=overlay_sha256 or not isinstance(state["completed"],list) or state["completed"]!=list(STEPS[:len(state["completed"])]):raise RuntimeError("retirement checkpoint does not match the immutable plan or values overlay")
  completed=state["completed"]
 for step in plan["steps"][len(completed):]:
  for entry in step["commands"]:
   try:result=runner(entry["argv"],shell=False,capture_output=True,timeout=entry["timeout_seconds"],check=False)
   except (OSError,subprocess.TimeoutExpired) as error:raise RuntimeError(f"retirement step could not complete: {step['id']}") from error
   if result.returncode!=0:raise RuntimeError(f"retirement step failed: {step['id']}")
  completed.append(step["id"]);state={"schema_version":1,"run_id":plan["run_id"],"plan_sha256":digest,"overlay_sha256":overlay_sha256,"completed":completed}
  checkpoint.parent.mkdir(parents=True,exist_ok=True);temporary=None
  try:
   with tempfile.NamedTemporaryFile("wb",dir=checkpoint.parent,prefix=checkpoint.name+".",delete=False) as handle:
    temporary=Path(handle.name);os.chmod(temporary,0o600);handle.write(canonical(state)+b"\n");handle.flush();os.fsync(handle.fileno())
   os.replace(temporary,checkpoint);temporary=None
  finally:
   if temporary and temporary.exists():temporary.unlink()
 return {"passed":True,"run_id":plan["run_id"],"completed_steps":len(completed),"plan_sha256":digest,"overlay_sha256":overlay_sha256}
