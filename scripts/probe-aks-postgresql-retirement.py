"""Bounded kubectl/Helm adapters for AKS PostgreSQL restart and rollback probes."""
import argparse,json,subprocess,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import postgresql_free_smoke as smoke  # noqa: E402

MAX_OUTPUT=16*1024*1024

def run(argv,*,json_output=False,timeout=600,runner=subprocess.run):
    result=runner(argv,shell=False,capture_output=True,timeout=timeout,check=False)
    stdout=result.stdout if isinstance(result.stdout,bytes) else str(result.stdout).encode()
    stderr=result.stderr if isinstance(result.stderr,bytes) else str(result.stderr).encode()
    if len(stdout)>MAX_OUTPUT or len(stderr)>MAX_OUTPUT:raise RuntimeError("AKS probe output is oversized")
    if result.returncode:raise RuntimeError(f"AKS probe command failed: {Path(argv[0]).name}")
    if json_output:
        try:return json.loads(stdout)
        except (UnicodeDecodeError,ValueError) as error:raise RuntimeError("AKS probe returned invalid JSON") from error
    return stdout.decode().strip()

def kube(context,namespace,*args,**kwargs):return run(["kubectl","--context",context,"-n",namespace,*args],**kwargs)

def workload(context,namespace,selector,action,*args):
    pods=kube(context,namespace,"get","pods","-l",selector,"-o","json",json_output=True)
    items=pods.get("items") if isinstance(pods,dict) else None
    if not isinstance(items,list) or len(items)!=1:raise RuntimeError("AKS probe requires exactly one API pod")
    pod=items[0].get("metadata",{}).get("name")
    if not pod:raise RuntimeError("AKS API pod identity is missing")
    return kube(context,namespace,"exec",pod,"--","python","-m","services.aks_retirement_live_probe",action,*args,json_output=True)

def pg_unavailable(context,namespace,retain=False):
    if retain:
        sts=kube(context,namespace,"get","statefulset","kaveon-postgres","-o","json",json_output=True)
        svc=kube(context,namespace,"get","service","kaveon-postgres","-o","json",json_output=True)
        pods=kube(context,namespace,"get","pod","-l","app=kaveon-postgres","-o","json",json_output=True)
        items=pods.get("items",[])
        if sts.get("metadata",{}).get("name")!="kaveon-postgres" or svc.get("metadata",{}).get("name")!="kaveon-postgres" or len(items)!=1:
            raise RuntimeError("retained PostgreSQL resources are incomplete")
        if not all(s.get("ready") is True for s in (items[0].get("status",{}).get("containerStatuses") or [])):
            raise RuntimeError("retained PostgreSQL pod is not ready")
        policy=kube(context,namespace,"get","networkpolicy","kaveon-portal-postgres-ingress","-o","json",json_output=True)
        if policy.get("spec",{}).get("ingress") != []:
            raise RuntimeError("retained PostgreSQL ingress is not isolated")
        return {"postgresql_unavailable":True,"postgresql_resources_retained":True,"postgresql_ingress_isolated":True}
    for resource in ("statefulset","service"):
        value=kube(context,namespace,"get",resource,"-o","json",json_output=True)
        names={item.get("metadata",{}).get("name") for item in value.get("items",[])}
        if "kaveon-postgres" in names:raise RuntimeError(f"PostgreSQL {resource} remains available")
    value=kube(context,namespace,"get","pod","-l","app=kaveon-postgres","-o","json",json_output=True)
    if value.get("items") != []:raise RuntimeError("PostgreSQL pod remains available")
    return {"postgresql_unavailable":True}

def pods(context,namespace,selector):
    return kube(context,namespace,"get","pods","-l",selector,"-o","json",json_output=True)

def restart(context,namespace):
    for deployment in ("kaveon-api","kaveon-portal"):
        kube(context,namespace,"rollout","restart",f"deployment/{deployment}")
        kube(context,namespace,"rollout","status",f"deployment/{deployment}","--timeout=10m",timeout=660)

def smoke_report(context,namespace,selector,identity,dataset_id,question):
    report=workload(context,namespace,selector,"smoke-report","--identity",identity,
                    "--dataset-id",dataset_id,"--question",question)
    return smoke.verify(report)

def target_fence(context,namespace):
    kube(context,namespace,"scale","deployment/kaveon-api","--replicas=0")
    kube(context,namespace,"wait","--for=delete","pod","-l","app=kaveon-api,!job","--timeout=10m",timeout=660)
    deployment=kube(context,namespace,"get","deployment","kaveon-api","-o","json",json_output=True)
    if deployment.get("spec",{}).get("replicas")!=0 or deployment.get("status",{}).get("replicas",0)!=0:
        raise RuntimeError("KaveonDB target writes are not fenced")
    return {"target_writes_fenced":True}

def rollback(helm,context,namespace,release,revision):
    run([helm,"rollback",release,str(revision),"--kube-context",context,"-n",namespace,"--wait","--timeout","15m","--cleanup-on-fail"],timeout=960)
    return {"cutover_revision":f"{release}@{revision}","rollback_operation_count":1}

def source_probe(context,namespace,write):
    sql=("BEGIN; CREATE TEMP TABLE kaveon_retirement_write_probe(value integer); "
         "INSERT INTO kaveon_retirement_write_probe VALUES (1); ROLLBACK; SELECT 1;" if write else "SELECT 1;")
    result=kube(context,namespace,"exec","statefulset/kaveon-postgres","--","psql","-U","kaveon","-d","kaveonmeta","-v","ON_ERROR_STOP=1","-Atqc",sql)
    if not result.endswith("1"):raise RuntimeError("PostgreSQL source probe failed")
    if not write:
        health=workload(context,namespace,"app=kaveon-api,!job","api-health","--authority","postgresql")
        if health!={"api_healthy":True,"authority":"postgresql"}:raise RuntimeError("restored API health probe failed")
    return {"source_writes_restored" if write else "source_reads_restored":True}

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--context",default="kaveon-test-aks");parser.add_argument("--namespace",default="kaveon")
    commands=parser.add_subparsers(dest="command",required=True)
    select=lambda name: commands.add_parser(name).add_argument("--selector",required=True)
    select("pods");select("state-inventory")
    smoke_command=commands.add_parser("smoke-report");smoke_command.add_argument("--selector",required=True);smoke_command.add_argument("--identity",required=True);smoke_command.add_argument("--dataset-id",required=True);smoke_command.add_argument("--question",required=True)
    pg=commands.add_parser("postgresql-unavailable");pg.add_argument("--retain",action="store_true")
    commands.add_parser("restart");commands.add_parser("target-fence")
    health=commands.add_parser("api-health");health.add_argument("--authority",required=True,choices=("kaveondb","postgresql"));health.add_argument("--selector",default="app=kaveon-api,!job")
    rb=commands.add_parser("rollback");rb.add_argument("--helm",required=True);rb.add_argument("--release",required=True);rb.add_argument("--revision",required=True,type=int)
    commands.add_parser("source-reads");commands.add_parser("source-writes")
    args=parser.parse_args()
    try:
        if args.command=="pods":value=pods(args.context,args.namespace,args.selector)
        elif args.command=="state-inventory":value=workload(args.context,args.namespace,args.selector,"state-inventory")
        elif args.command=="smoke-report":value=smoke_report(args.context,args.namespace,args.selector,args.identity,args.dataset_id,args.question)
        elif args.command=="postgresql-unavailable":value=pg_unavailable(args.context,args.namespace,args.retain)
        elif args.command=="restart":restart(args.context,args.namespace);value=None
        elif args.command=="target-fence":value=target_fence(args.context,args.namespace)
        elif args.command=="api-health":value=workload(args.context,args.namespace,args.selector,"api-health","--authority",args.authority)
        elif args.command=="rollback":value=rollback(args.helm,args.context,args.namespace,args.release,args.revision)
        else:value=source_probe(args.context,args.namespace,args.command=="source-writes")
        if value is not None:print(json.dumps(value,sort_keys=True,separators=(",",":")))
        return 0
    except (OSError,RuntimeError,ValueError,subprocess.TimeoutExpired) as error:
        print(json.dumps({"passed":False,"error":str(error)},sort_keys=True,separators=(",",":")));return 1

if __name__=="__main__":raise SystemExit(main())
