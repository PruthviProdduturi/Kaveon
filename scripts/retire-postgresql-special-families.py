"""Delete rebuildable PostgreSQL state after fencing and emit strict reports."""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_special_family_retirement as retirement  # noqa: E402

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--dlm-bundle", required=True, type=Path)
parser.add_argument("--context-rebuild", required=True, type=Path)
parser.add_argument("--write-fence-observation", required=True, type=Path)
parser.add_argument("--expected-source-counts", required=True, type=Path)
parser.add_argument("--lossless-baseline", required=True, type=Path)
parser.add_argument("--lossless-migration-evidence", required=True, type=Path)
parser.add_argument("--output-directory", required=True, type=Path)
parser.add_argument("--max-age-hours", type=int, default=1)
args = parser.parse_args()

def load(path):
    if not path.is_file() or path.stat().st_size > 4 * 1024 * 1024:
        raise RuntimeError(f"missing or oversized input: {path.name}")
    return json.loads(path.read_text(encoding="utf-8"))

try:
    result = retirement.run(bundle=load(args.dlm_bundle),
        rebuild=load(args.context_rebuild),
        fence_observation=load(args.write_fence_observation),
        expected_counts=load(args.expected_source_counts),
        baseline=load(args.lossless_baseline),
        migration_evidence=load(args.lossless_migration_evidence),
        output_directory=args.output_directory, max_age_hours=args.max_age_hours)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
except Exception as error:
    print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                     separators=(",", ":")))
    raise SystemExit(1)
