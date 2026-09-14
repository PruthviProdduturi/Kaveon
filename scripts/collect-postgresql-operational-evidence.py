"""Build retirement gates only from complete, integrity-bound live observations."""

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_operational_evidence as evidence  # noqa: E402


def _write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--observations", required=True, type=Path)
    parser.add_argument("--gates", required=True, type=Path)
    parser.add_argument("--operational", required=True, type=Path)
    parser.add_argument("--max-rollback-seconds", type=int, default=900)
    parser.add_argument("--max-age-hours", type=int, default=24)
    args = parser.parse_args()
    try:
        gates, operational = evidence.collect(
            args.observations, now=datetime.now(timezone.utc),
            max_age_hours=args.max_age_hours,
            max_rollback_seconds=args.max_rollback_seconds)
        _write(args.gates, gates)
        _write(args.operational, operational)
        print(json.dumps({"passed": True, "gate_count": len(gates),
                          "receipt_set_sha256": operational["receipt_set_sha256"]}, sort_keys=True))
        return 0
    except (OSError, RuntimeError, ValueError) as error:
        print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
