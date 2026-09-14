"""Build the fail-closed context-cache retirement family report."""

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "api"))

from services import context_cache_retirement  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-age-hours", type=int, default=1)
    args = parser.parse_args()
    try:
        if (not args.evidence.is_file()
                or args.evidence.stat().st_size > context_cache_retirement.MAX_EVIDENCE_BYTES):
            raise RuntimeError("context-cache evidence is missing or oversized")
        observation = json.loads(args.evidence.read_text(encoding="utf-8"))
        report = context_cache_retirement.build_report(
            observation, now=datetime.now(timezone.utc), max_age_hours=args.max_age_hours)
        encoded = json.dumps(report, sort_keys=True, indent=2) + "\n"
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(args.output.name + ".tmp")
        with temporary.open("w", encoding="utf-8", newline="\n") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, args.output)
        try:
            directory_fd = os.open(args.output.parent, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        except OSError:
            pass
        print(json.dumps({"family": "context_cache", "status": "passed",
                          "report": str(args.output)}))
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"family": "context_cache", "status": "failed",
                          "error": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
