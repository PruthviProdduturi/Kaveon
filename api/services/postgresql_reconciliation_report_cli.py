"""Operator entrypoint for strict PostgreSQL reconciliation report collection."""

import argparse
import json
from pathlib import Path

from services.postgresql_reconciliation_report_collector import collect


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify completed checkpoints against live KaveonDB and emit 16 family reports")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output-directory", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(collect(args.manifest, args.output_directory), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
