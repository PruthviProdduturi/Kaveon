import json
from types import SimpleNamespace
import pytest
from services import restart_rollback_probes as probes
from services import kaveondb_recovery_evidence as recovery

RECORDS=[{"kind":"dataset","id":"1","revision":1,"document_sha256":"a"*64}]
def spec(name):return {"argv":["kubectl",name],"timeout_seconds":30}
def pod(uid):return {"items":[{"metadata":{"uid":uid},"status":{"containerStatuses":[{"ready":True}]}}]}
def runner(values):
 def call(argv,**kwargs):
  assert kwargs["shell"] is False
  value=values[argv[1]];return SimpleNamespace(returncode=0,stdout=json.dumps(value).encode() if value is not None else b"",stderr=b"")
 return call
def restart_manifest():return {name:spec(name) for name in ("state_before","api_pods_before","studio_pods_before","postgresql_unavailable","restart","api_pods_after","studio_pods_after","state_after","service_probes")}

def test_restart_requires_distinct_ready_pods_pg_down_and_stable_state():
 values={"state_before":RECORDS,"api_pods_before":pod("api-1"),"studio_pods_before":pod("studio-1"),"postgresql_unavailable":{"postgresql_unavailable":True},"restart":None,"api_pods_after":pod("api-2"),"studio_pods_after":pod("studio-2"),"state_after":RECORDS,"service_probes":{"passed":True,"probe_count":16}}
 result=probes.restart(restart_manifest(),runner(values))
 assert result["api_restarted"] and result["postgresql_unavailable"] and result["state_record_count_after"]==1

def test_restart_fails_when_pod_was_not_replaced():
 values={"state_before":RECORDS,"api_pods_before":pod("same"),"studio_pods_before":pod("studio-1"),"postgresql_unavailable":{"postgresql_unavailable":True},"restart":None,"api_pods_after":pod("same"),"studio_pods_after":pod("studio-2"),"state_after":RECORDS,"service_probes":{"passed":True,"probe_count":16}}
 with pytest.raises(RuntimeError,match="not replaced"):probes.restart(restart_manifest(),runner(values))

def test_rollback_enforces_control_and_restored_probes():
 identity=recovery.state_identity(RECORDS);control={"cutover_revision":"api@abc","expected_state_sha256":identity["state_sha256"],"max_operations":20,"max_duration_seconds":60}
 manifest={name:spec(name) for name in ("state_before","target_fence","rollback","source_reads","source_writes","state_after")}
 values={"state_before":RECORDS,"target_fence":{"target_writes_fenced":True},"rollback":{"cutover_revision":"api@abc","rollback_operation_count":4},"source_reads":{"source_reads_restored":True},"source_writes":{"source_writes_restored":True},"state_after":RECORDS}
 ticks=iter([10.0,15.2]);result=probes.rollback(manifest,control,runner(values),lambda:next(ticks))
 assert result["duration_seconds"]==6 and result["rollback_operation_count"]==4

def test_rollback_rejects_operation_overrun():
 identity=recovery.state_identity(RECORDS);control={"cutover_revision":"api@abc","expected_state_sha256":identity["state_sha256"],"max_operations":2,"max_duration_seconds":60}
 manifest={name:spec(name) for name in ("state_before","target_fence","rollback","source_reads","source_writes","state_after")}
 values={"state_before":RECORDS,"target_fence":{"target_writes_fenced":True},"rollback":{"cutover_revision":"api@abc","rollback_operation_count":3}}
 with pytest.raises(RuntimeError,match="control"):probes.rollback(manifest,control,runner(values),lambda:1)

def test_shell_interpreters_are_rejected():
 with pytest.raises(RuntimeError,match="unsafe"):probes.command({"argv":["pwsh","-c","kubectl delete pod"],"timeout_seconds":30})
 with pytest.raises(RuntimeError,match="unsafe"):probes.command({"argv":["python","-c","print('fake')"],"timeout_seconds":30})
