import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_live_inventory as inventory  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
report = inventory.collect()
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"passed": True, "table_count": len(report["discovered_tables"]),
                  "report_sha256": report["report_sha256"]}, sort_keys=True))
