"""API-image entrypoint for strict DLM generation retirement evidence."""
import argparse,json,os,tempfile
from datetime import datetime,timezone
from pathlib import Path
from services import dlm_generation_retirement

def load(path,label,limit):
 if not path.is_file() or path.stat().st_size>limit:raise RuntimeError(f"{label} is missing or oversized")
 value=json.loads(path.read_text(encoding="utf-8"))
 if not isinstance(value,dict):raise RuntimeError(f"{label} is invalid")
 return value
def atomic(path,value):
 path=path.resolve();path.parent.mkdir(parents=True,exist_ok=True);temporary=None
 try:
  with tempfile.NamedTemporaryFile("wb",dir=path.parent,prefix=path.name+".",delete=False) as h:temporary=Path(h.name);os.chmod(temporary,0o600);h.write(json.dumps(value,sort_keys=True,separators=(",",":")).encode());h.flush();os.fsync(h.fileno())
  os.replace(temporary,path)
 finally:
  if temporary and temporary.exists():temporary.unlink()
def main():
 p=argparse.ArgumentParser(description="Verify compiled DLM migration plus live legacy-state deletion")
 p.add_argument("--bundle",required=True,type=Path);p.add_argument("--retirement-evidence",required=True,type=Path);p.add_argument("--output",required=True,type=Path);p.add_argument("--max-age-hours",type=int,default=1);a=p.parse_args()
 report=dlm_generation_retirement.build_report(load(a.bundle,"DLM migration bundle",4*1024*1024),load(a.retirement_evidence,"DLM retirement evidence",dlm_generation_retirement.MAX_EVIDENCE_BYTES),now=datetime.now(timezone.utc),max_age_hours=a.max_age_hours);atomic(a.output,report);print(json.dumps({"family":"dlm_generation","status":"passed","report":str(a.output)}));return 0
if __name__=="__main__":raise SystemExit(main())
