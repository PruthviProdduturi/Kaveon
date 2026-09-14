import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import user_theme_backfill_operation as operation  # noqa: E402
from services import migration_checkpoint_store  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument("--checkpoint", required=True, type=Path)
parser.add_argument("--resume", action="store_true")
parser.add_argument("--apply", action="store_true")
args = parser.parse_args()
print(json.dumps(migration_checkpoint_store.run(operation, args.checkpoint, apply=args.apply,
    invoke=lambda checkpoint: operation.run(checkpoint, apply=args.apply, resume=args.resume)), sort_keys=True))
