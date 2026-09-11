"""Assemble verified local reconciliation reports; disabled by default."""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_evidence_collector  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reports", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        evidence = postgresql_evidence_collector.collect(args.reports)
        encoded = json.dumps(evidence, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
        print(json.dumps({"collected": len(evidence["families"]), "output": str(args.output)}))
        return 0
    except (OSError, RuntimeError) as error:
        print(json.dumps({"collected": 0, "error": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
