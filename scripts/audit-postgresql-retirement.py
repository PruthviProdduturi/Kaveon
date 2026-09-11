"""Evaluate credential-free PostgreSQL retirement reconciliation evidence."""

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_retirement_gate  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-age-hours", type=int, default=24)
    args = parser.parse_args()
    try:
        evidence = json.loads(args.evidence.read_text(encoding="utf-8"))
        audit = postgresql_retirement_gate.evaluate(
            evidence, now=datetime.now(timezone.utc), max_age_hours=args.max_age_hours
        )
        encoded = json.dumps(audit, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
        print(encoded)
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"gate": "postgresql-retirement-parity", "passed": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
