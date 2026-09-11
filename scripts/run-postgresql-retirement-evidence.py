"""Collect and evaluate PostgreSQL retirement evidence as one fail-closed step.

This command only reads already-produced, integrity-bound reports.  It never
connects to PostgreSQL, changes a fence, or performs a cutover.  Both output
files are written only after every authority family and global retirement gate
passes validation at the same explicit evaluation time.
"""

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_evidence_collector  # noqa: E402
from services import postgresql_retirement_gate  # noqa: E402


def _parse_now(value: str) -> datetime:
    if not value.endswith("Z"):
        raise ValueError("--now must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise ValueError("--now is not a valid RFC3339 timestamp") from error
    if parsed.utcoffset() != timezone.utc.utcoffset(parsed):
        raise ValueError("--now must be an RFC3339 UTC timestamp ending in Z")
    return parsed


def _write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def run(reports: Path, evidence_output: Path, audit_output: Path, *, now: datetime, max_age_hours: int) -> dict:
    evidence = postgresql_evidence_collector.collect(reports)
    audit = postgresql_retirement_gate.evaluate(
        evidence, now=now, max_age_hours=max_age_hours
    )
    # Publish only after validation. A stale or incomplete live run therefore
    # cannot produce a passing artifact.
    _write_json(evidence_output, evidence)
    _write_json(audit_output, audit)
    return {
        "passed": audit["passed"],
        "authority_family_count": audit["authority_family_count"],
        "global_gate_count": len(audit["gates"]),
        "evidence": str(evidence_output),
        "audit": str(audit_output),
        "checked_at": audit["checked_at"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--audit", required=True, type=Path)
    parser.add_argument(
        "--now",
        type=_parse_now,
        help="explicit RFC3339 UTC evaluation time (defaults to current UTC)",
    )
    parser.add_argument("--max-age-hours", type=int, default=24)
    args = parser.parse_args()
    try:
        now = args.now or datetime.now(timezone.utc)
        result = run(
            args.reports,
            args.evidence,
            args.audit,
            now=now,
            max_age_hours=args.max_age_hours,
        )
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"passed": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
