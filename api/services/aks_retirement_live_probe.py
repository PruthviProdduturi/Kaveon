"""Content-free probes executed inside the API workload during AKS retirement."""
import argparse,json,os
from urllib.request import Request,urlopen

from services import kaveondb_backup,postgresql_free_smoke


def state_inventory():
    records,_ = kaveondb_backup.product_inventory(actor="kaveon-retirement-probe")
    return records


def api_health(authority: str):
    if authority not in {"kaveondb","postgresql"}:
        raise RuntimeError("expected API authority is invalid")
    request=Request("http://127.0.0.1:8080/api/health",headers={"Accept":"application/json"})
    with urlopen(request,timeout=10) as response:
        if response.status != 200 or int(response.headers.get("Content-Length","0") or 0)>1024*1024:
            raise RuntimeError("API health request failed")
        payload=response.read(1024*1024+1)
    if len(payload)>1024*1024:raise RuntimeError("API health response is oversized")
    value=json.loads(payload)
    if value.get("status")!="healthy" or value.get("authority")!=authority:
        raise RuntimeError("API health authority did not match")
    required=((value.get("checks") or {}).get("postgresql") or {}).get("required")
    if required is not (authority=="postgresql"):
        raise RuntimeError("API PostgreSQL requirement did not match authority")
    return {"api_healthy":True,"authority":authority}


def smoke_report(identity: str,dataset_id: str,question: str):
    if os.getenv("KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE")!="true":
        raise RuntimeError("smoke report requires restart rehearsal mode")
    with patch_env("KAVEON_POSTGRESQL_FREE_SMOKE_ENABLED","true"):
        return postgresql_free_smoke.collect("http://127.0.0.1:8080",identity,
            os.getenv("KAVEON_PROXY_SECRET",""),question,dataset_id)


class patch_env:
    def __init__(self,key,value):self.key=key;self.value=value;self.previous=None
    def __enter__(self):self.previous=os.environ.get(self.key);os.environ[self.key]=self.value
    def __exit__(self,*_args):
        if self.previous is None:os.environ.pop(self.key,None)
        else:os.environ[self.key]=self.previous


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest="command",required=True)
    commands.add_parser("state-inventory")
    health=commands.add_parser("api-health");health.add_argument("--authority",required=True,choices=("kaveondb","postgresql"))
    smoke=commands.add_parser("smoke-report");smoke.add_argument("--identity",required=True);smoke.add_argument("--dataset-id",required=True);smoke.add_argument("--question",required=True)
    args=parser.parse_args()
    try:
        if args.command=="state-inventory":value=state_inventory()
        elif args.command=="api-health":value=api_health(args.authority)
        else:value=smoke_report(args.identity,args.dataset_id,args.question)
    except (OSError,RuntimeError,ValueError) as error:
        print(json.dumps({"passed":False,"error":str(error)},sort_keys=True,separators=(",",":")));return 1
    print(json.dumps(value,sort_keys=True,separators=(",",":")));return 0


if __name__=="__main__":raise SystemExit(main())
