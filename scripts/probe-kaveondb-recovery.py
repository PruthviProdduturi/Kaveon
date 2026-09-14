"""Execute live KaveonDB restart or bounded rollback operational probes."""
import argparse,json,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import restart_rollback_probes as probes  # noqa: E402
MAX_MANIFEST=256*1024
def load(path):
 if not path.is_file() or path.stat().st_size>MAX_MANIFEST:raise RuntimeError("probe manifest is missing or oversized")
 try:value=json.loads(path.read_text(encoding="utf-8"))
 except (OSError,ValueError) as error:raise RuntimeError("probe manifest is invalid") from error
 return value
def main():
 parser=argparse.ArgumentParser(description=__doc__);commands=parser.add_subparsers(dest="command",required=True)
 restart=commands.add_parser("restart-recovery");restart.add_argument("--manifest",required=True,type=Path)
 rollback=commands.add_parser("rollback");rollback.add_argument("--manifest",required=True,type=Path);rollback.add_argument("--control",required=True,type=Path)
 args=parser.parse_args()
 try:
  result=probes.restart(load(args.manifest)) if args.command=="restart-recovery" else probes.rollback(load(args.manifest),load(args.control))
  print(json.dumps(result,sort_keys=True,separators=(",",":")));return 0
 except (OSError,RuntimeError,ValueError) as error:
  print(json.dumps({"passed":False,"error":str(error)},sort_keys=True,separators=(",",":")));return 1
if __name__=="__main__":raise SystemExit(main())
