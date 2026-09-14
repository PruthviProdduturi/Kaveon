import argparse,json,os,tempfile
from pathlib import Path
from services import postgresql_free_smoke
def main():
 p=argparse.ArgumentParser(description="Black-box PostgreSQL-free API restart qualification")
 p.add_argument("--base-url",required=True);p.add_argument("--identity",required=True);p.add_argument("--question",required=True);p.add_argument("--dataset-id",required=True);p.add_argument("--ca-cert",type=Path);p.add_argument("--output",required=True,type=Path);a=p.parse_args()
 secret=os.getenv("KAVEON_PROXY_SECRET","");report=postgresql_free_smoke.collect(a.base_url,a.identity,secret,a.question,a.dataset_id,ca_cert=str(a.ca_cert) if a.ca_cert else None)
 a.output=a.output.resolve();a.output.parent.mkdir(parents=True,exist_ok=True);temporary=None
 try:
  with tempfile.NamedTemporaryFile("wb",dir=a.output.parent,prefix=a.output.name+".",delete=False) as h:temporary=Path(h.name);os.chmod(temporary,0o600);h.write(json.dumps(report,sort_keys=True,separators=(",",":")).encode());h.flush();os.fsync(h.fileno())
  os.replace(temporary,a.output)
 finally:
  if temporary and temporary.exists():temporary.unlink()
 print(json.dumps({"status":"passed","checks":report["check_count"],"report":str(a.output)}));return 0
if __name__=="__main__":raise SystemExit(main())
