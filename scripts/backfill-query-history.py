import argparse,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services import query_history_backfill_operation as operation
from services import migration_checkpoint_store
def main():
 parser=argparse.ArgumentParser();parser.add_argument("--checkpoint",type=Path,required=True);parser.add_argument("--resume",action="store_true");parser.add_argument("--apply",action="store_true");args=parser.parse_args();print(migration_checkpoint_store.run(operation,args.checkpoint,apply=args.apply,invoke=lambda checkpoint: operation.run(checkpoint,apply=args.apply,resume=args.resume)));return 0
if __name__=="__main__":raise SystemExit(main())
