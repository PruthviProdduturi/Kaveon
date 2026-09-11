"""Validate and optionally emit the PostgreSQL cutover dependency inventory."""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "api"))
from services import postgresql_dependency_inventory  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        inventory = postgresql_dependency_inventory.scan(ROOT / "api")
        encoded = json.dumps(inventory, sort_keys=True, separators=(",", ":"))
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded + "\n", encoding="utf-8")
        print(encoded)
        return 0
    except (OSError, RuntimeError) as error:
        print(json.dumps({"passed": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
