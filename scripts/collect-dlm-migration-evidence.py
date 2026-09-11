import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import dlm_migration_evidence as evidence  # noqa: E402

parser = argparse.ArgumentParser()
for name in ("definition-checkpoint", "run-checkpoint", "artifact-receipts",
             "definition-report", "run-report", "target-observations"):
    parser.add_argument("--" + name, required=True, type=Path)
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
bundle = evidence.collect(args.definition_checkpoint, args.run_checkpoint, args.artifact_receipts,
                          args.definition_report, args.run_report, args.target_observations,
                          collected_at=datetime.now(timezone.utc))
args.output.write_text(json.dumps(bundle, sort_keys=True, separators=(",", ":")), encoding="utf-8")
