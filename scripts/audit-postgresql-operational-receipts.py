"""Report every missing or invalid PostgreSQL retirement receipt without mutating evidence."""

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_operational_evidence as evidence  # noqa: E402


def _now(value: str) -> datetime:
    if not value.endswith("Z"):
        raise argparse.ArgumentTypeError("--now must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise argparse.ArgumentTypeError("--now is not a valid RFC3339 timestamp") from error
    return parsed


def audit(directory: Path, *, now: datetime, max_age_hours: int,
          max_rollback_seconds: int) -> dict:
    results = []
    for gate in evidence.GATES:
        path = directory / f"{gate}.json"
        try:
            evidence.load_receipt(path, gate, now=now, max_age_hours=max_age_hours,
                                  max_rollback_seconds=max_rollback_seconds)
            status, error = "passed", None
        except (OSError, RuntimeError, ValueError) as failure:
            status, error = ("missing" if not path.is_file() else "failed"), str(failure)
        item = {"gate": gate, "status": status, "path": str(path)}
        if error is not None:
            item["error"] = error
        results.append(item)
    passed = all(item["status"] == "passed" for item in results)
    return {"schema_version": 1, "passed": passed,
            "checked_at": now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z"),
            "gate_count": len(results), "passed_count": sum(
                item["status"] == "passed" for item in results), "gates": results}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--observations", required=True, type=Path)
    parser.add_argument("--now", type=_now)
    parser.add_argument("--max-age-hours", type=int, default=24)
    parser.add_argument("--max-rollback-seconds", type=int, default=900)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    result = audit(args.observations, now=args.now or datetime.now(timezone.utc),
                   max_age_hours=args.max_age_hours,
                   max_rollback_seconds=args.max_rollback_seconds)
    encoded = json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(args.output.name + ".tmp")
        temporary.write_text(encoded, encoding="utf-8")
        temporary.replace(args.output)
    print(encoded, end="")
    return 0 if result["passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
