"""Execute a live shadow-parity or PostgreSQL write-fence retirement probe."""

import argparse,json,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import live_cutover_probes as probes  # noqa: E402


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest="command",required=True)
    shadow=commands.add_parser("shadow-parity");shadow.add_argument("--reports",required=True,type=Path);shadow.add_argument("--max-age-hours",type=int,default=24)
    write=commands.add_parser("write-fence");write.add_argument("--deployment-revision",required=True)
    args=parser.parse_args()
    try:
        result=(probes.shadow_parity(args.reports,max_age_hours=args.max_age_hours) if args.command=="shadow-parity"
                else probes.write_fence(args.deployment_revision))
        print(json.dumps(result,sort_keys=True,separators=(",",":")))
        return 0
    except (OSError,RuntimeError,ValueError) as error:
        print(json.dumps({"passed":False,"error":str(error)},sort_keys=True,separators=(",",":")))
        return 1


if __name__=="__main__":raise SystemExit(main())
