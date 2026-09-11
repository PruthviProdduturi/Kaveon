import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import dlm_run_backfill_operation as operation  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument("--checkpoint", required=True, type=Path)
parser.add_argument("--artifact-root", required=True, type=Path)
parser.add_argument("--resume", action="store_true")
parser.add_argument("--apply", action="store_true")
parser.add_argument("--client-factory", help="module:function returning a create-only ADLS client")
args = parser.parse_args()
publisher = None
if args.apply:
    if not args.client_factory or ":" not in args.client_factory:
        parser.error("--apply requires --client-factory module:function")
    module_name, factory_name = args.client_factory.rsplit(":", 1)
    client = getattr(importlib.import_module(module_name), factory_name)()
    from services.dlm_artifact_publisher import Publisher  # noqa: E402
    publisher = Publisher(client, args.artifact_root)
print(json.dumps(operation.run(args.checkpoint, args.artifact_root,
                               apply=args.apply, resume=args.resume, publisher=publisher), sort_keys=True))
