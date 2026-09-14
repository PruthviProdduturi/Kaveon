"""Restore a reviewed special-family bundle during bounded PostgreSQL rollback."""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_special_family_restore as restore  # noqa: E402

MAX_INPUT_BYTES = 64 * 1024 * 1024


def load(path):
    if not path.is_file() or path.stat().st_size > MAX_INPUT_BYTES:
        raise RuntimeError(f"missing or oversized restore input: {path.name}")
    return json.loads(path.read_text(encoding="utf-8"))


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--bundle", required=True, type=Path)
parser.add_argument("--context-deletion-evidence", required=True, type=Path)
parser.add_argument("--dlm-deletion-evidence", required=True, type=Path)
parser.add_argument("--expected-bundle-sha256", required=True)
parser.add_argument("--cutover-revision", required=True)
parser.add_argument("--max-operations", type=int, default=10_000)
parser.add_argument("--max-duration-seconds", type=int, default=900)
args = parser.parse_args()

try:
    result = restore.restore(load(args.bundle), {
        "context": load(args.context_deletion_evidence),
        "dlm": load(args.dlm_deletion_evidence),
    }, expected_bundle_sha256=args.expected_bundle_sha256,
       cutover_revision=args.cutover_revision, max_operations=args.max_operations,
       max_duration_seconds=args.max_duration_seconds)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
except Exception as error:
    print(json.dumps({"restored": False, "error": str(error)}, sort_keys=True,
                     separators=(",", ":")))
    raise SystemExit(1)
