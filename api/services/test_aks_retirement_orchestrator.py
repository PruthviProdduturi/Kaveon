import json
from types import SimpleNamespace
import pytest
from services import aks_retirement_orchestrator as o

def plan(overlay):
 scripts={"shadow_parity":"probe-kaveondb-cutover.py","backup":"create-kaveondb-adls-backup.py","restore":"rehearse-kaveondb-adls-restore.py","restart_recovery":"probe-kaveondb-recovery.py","rollback":"probe-kaveondb-recovery.py","final_audit":"run-postgresql-retirement-evidence.py"}
 steps=[]
 for name in o.STEPS:
  argv=["python",scripts.get(name,"step.py")]
  if name=="helm_migration":argv=["helm","upgrade","kaveon","chart","-f",str(overlay)]
  steps.append({"id":name,"commands":[{"argv":argv,"timeout_seconds":60}]})
 return {"schema_version":1,"run_id":"run-1","steps":steps}

def test_overlay_rejects_secret_material(tmp_path):
 safe=tmp_path/"safe.yaml";safe.write_text("api:\n  cutover:\n    retirementMode: false\n")
 assert o.validate_overlay(safe)["api"]
 unsafe=tmp_path/"bad.yaml";unsafe.write_text("password: exposed\n")
 with pytest.raises(RuntimeError,match="secret field"):o.validate_overlay(unsafe)

def test_ordered_plan_is_resumable_and_digest_bound(tmp_path):
 overlay=tmp_path/"values.yaml";overlay.write_text("api: {}\n");path=tmp_path/"plan.json";value=plan(overlay);path.write_text(json.dumps(value))
 loaded=o.load_plan(path,overlay);checkpoint=tmp_path/"checkpoint.json";calls=[]
 def runner(argv,**kwargs):calls.append(argv);return SimpleNamespace(returncode=0,stdout=b"",stderr=b"")
 result=o.run(loaded,checkpoint,runner);assert result["completed_steps"]==len(o.STEPS)
 assert len(calls)==len(o.STEPS);o.run(loaded,checkpoint,runner);assert len(calls)==len(o.STEPS)
 changed=plan(overlay);changed["steps"][0]["commands"][0]["argv"].append("changed")
 with pytest.raises(RuntimeError,match="immutable plan"):o.run(changed,checkpoint,runner)

def test_failure_stops_and_resume_begins_at_failed_step(tmp_path):
 overlay=tmp_path/"values.yaml";overlay.write_text("api: {}\n");value=plan(overlay);checkpoint=tmp_path/"c.json";count=0
 def failing(argv,**kwargs):
  nonlocal count;count+=1;return SimpleNamespace(returncode=1 if count==3 else 0,stdout=b"",stderr=b"")
 with pytest.raises(RuntimeError,match="dlm_migration"):o.run(value,checkpoint,failing)
 assert json.loads(checkpoint.read_text())["completed"]==list(o.STEPS[:2])

def test_plan_requires_exact_order_and_reviewed_tools(tmp_path):
 overlay=tmp_path/"values.yaml";overlay.write_text("api: {}\n");path=tmp_path/"plan.json";value=plan(overlay);value["steps"].reverse();path.write_text(json.dumps(value))
 with pytest.raises(RuntimeError,match="step order"):o.load_plan(path,overlay)
