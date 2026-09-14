import importlib.util,json
from pathlib import Path
import pytest

SCRIPT=Path(__file__).with_name("probe-aks-postgresql-retirement.py")
spec=importlib.util.spec_from_file_location("aks_probe",SCRIPT);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

def test_postgresql_unavailable_checks_named_workload_service_and_pods(monkeypatch):
    seen=[]
    def kube(_context,_namespace,*args,**_kwargs):
        seen.append(args)
        return {"items":[]}
    monkeypatch.setattr(module,"kube",kube)
    assert module.pg_unavailable("ctx","ns")=={"postgresql_unavailable":True}
    assert [call[1] for call in seen]==["statefulset","service","pod"]

@pytest.mark.parametrize("resource",("statefulset","service"))
def test_postgresql_unavailable_rejects_named_resource(monkeypatch,resource):
    def kube(_context,_namespace,*args,**_kwargs):
        return {"items":[{"metadata":{"name":"kaveon-postgres"}}]} if args[1]==resource else {"items":[]}
    monkeypatch.setattr(module,"kube",kube)
    with pytest.raises(RuntimeError,match="remains available"):module.pg_unavailable("ctx","ns")

def test_target_fence_scales_api_to_zero_and_observes_it(monkeypatch):
    calls=[]
    def kube(_context,_namespace,*args,**_kwargs):
        calls.append(args)
        return {"spec":{"replicas":0},"status":{}} if args[:3]==("get","deployment","kaveon-api") else ""
    monkeypatch.setattr(module,"kube",kube)
    assert module.target_fence("ctx","ns")=={"target_writes_fenced":True}
    assert calls[0]==("scale","deployment/kaveon-api","--replicas=0")
    assert calls[1][:3]==("wait","--for=delete","pod")

def test_source_write_probe_uses_rollback_scoped_temp_table(monkeypatch):
    captured=[]
    def kube(_context,_namespace,*args,**_kwargs):captured.append(args);return "1"
    monkeypatch.setattr(module,"kube",kube)
    assert module.source_probe("ctx","ns",True)=={"source_writes_restored":True}
    sql=captured[0][-1]
    assert "CREATE TEMP TABLE" in sql and "ROLLBACK" in sql and "product_" not in sql

def test_source_read_probe_requires_postgresql_api_health(monkeypatch):
    monkeypatch.setattr(module,"kube",lambda *_args,**_kwargs:"1")
    monkeypatch.setattr(module,"workload",lambda *_args:{"api_healthy":True,"authority":"postgresql"})
    assert module.source_probe("ctx","ns",False)=={"source_reads_restored":True}

def test_smoke_report_runs_in_api_workload_and_verifies_full_report(monkeypatch):
    report={"check_count":21,"marker":"report"};verified={"verified":True};calls=[]
    monkeypatch.setattr(module,"workload",lambda *args:(calls.append(args),report)[1])
    monkeypatch.setattr(module.smoke,"verify",lambda value:verified if value==report else None)
    assert module.smoke_report("ctx","ns","app=api","owner@example.com","15","question")==verified
    assert calls[0][-6:]==("--identity","owner@example.com","--dataset-id","15","--question","question")

def test_rollback_uses_fixed_helm_argument_array(monkeypatch):
    calls=[];monkeypatch.setattr(module,"run",lambda argv,**kwargs:calls.append((argv,kwargs)))
    assert module.rollback("helm.exe","ctx","ns","portal",5)=={"cutover_revision":"portal@5","rollback_operation_count":1}
    assert calls[0][0][:4]==["helm.exe","rollback","portal","5"]
