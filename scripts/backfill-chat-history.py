import argparse,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import chat_history_backfill_operation as operation
def main():
 parser=argparse.ArgumentParser();parser.add_argument("--checkpoint",type=Path,required=True);parser.add_argument("--resume",action="store_true");parser.add_argument("--apply",action="store_true");args=parser.parse_args();print(operation.run(args.checkpoint,apply=args.apply,resume=args.resume));return 0
if __name__=="__main__":raise SystemExit(main())
