"""Capture, resume, and reconcile the PostgreSQL dataset backfill."""

import argparse
import json
import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_ROOT / "api"))

from services import product_backfill_operation  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument(
        "--apply",
        action="store_true",
        help="write missing records to KaveonDB; also requires KAVEON_PRODUCT_MIGRATION_ENABLED=true",
    )
    args = parser.parse_args()
    report = product_backfill_operation.run(
        args.checkpoint, apply=args.apply, resume=args.resume
    )
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
