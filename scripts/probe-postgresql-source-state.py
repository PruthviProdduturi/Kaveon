"""Emit one structured live source-watermark or outbox-drain observation."""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_source_state_probe as probe  # noqa: E402


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--gate", required=True, choices=("source_watermark", "outbox_drain"))
args = parser.parse_args()
state = probe.collect()
if args.gate == "source_watermark":
    result = {"source_snapshot": state["source_snapshot"],
              "watermark_observed": state["watermark"]}
else:
    result = {"query_id": state["query_id"], "watermark": state["watermark"],
              "pending_before": state["pending_events"],
              "pending_after": state["pending_events"]}
print(json.dumps(result, sort_keys=True, separators=(",", ":")))
