"""Run a reviewed, resumable AKS PostgreSQL retirement rehearsal plan."""
import argparse,json,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import aks_retirement_orchestrator as orchestrator  # noqa: E402
def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--plan",required=True,type=Path);parser.add_argument("--values",required=True,type=Path);parser.add_argument("--checkpoint",required=True,type=Path);args=parser.parse_args()
 try:
  orchestrator.validate_overlay(args.values);plan=orchestrator.load_plan(args.plan,args.values);result=orchestrator.run(plan,args.checkpoint)
  print(json.dumps(result,sort_keys=True,separators=(",",":")));return 0
 except (OSError,RuntimeError,ValueError) as error:
  print(json.dumps({"passed":False,"error":str(error)},sort_keys=True,separators=(",",":")));return 1
if __name__=="__main__":raise SystemExit(main())
