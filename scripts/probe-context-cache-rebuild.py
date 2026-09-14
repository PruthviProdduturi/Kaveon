"""Build a deterministic, content-free KaveonDB context rebuild observation."""

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import product_backfill_operation  # noqa: E402
from services.postgresql_reconciliation_report_collector import _target_records  # noqa: E402

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--dataset-checkpoint", required=True, type=Path)
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()

if os.getenv("KAVEON_CONTEXT_CACHE_REBUILD_PROBE_ENABLED") != "true":
    raise SystemExit("context-cache rebuild probe requires explicit enablement")
snapshot, position, complete = product_backfill_operation.load_checkpoint(args.dataset_checkpoint)
if complete is not True or position != len(snapshot.records):
    raise SystemExit("dataset checkpoint is incomplete")

def observe():
    records, snapshot_id = _target_records("dataset")
    rows = sorted([{"id": row["id"], "generation": row.get("generation"),
                   "document_sha256": hashlib.sha256(json.dumps(
                       row["document"], sort_keys=True, separators=(",", ":"),
                        ensure_ascii=False).encode()).hexdigest()} for row in records
                  ], key=lambda row: row["id"])
    if any(type(row["generation"]) is not int or row["generation"] < 1 for row in rows):
        raise SystemExit("KaveonDB dataset generation is invalid")
    return snapshot_id, rows

first_snapshot, first = observe()
second_snapshot, second = observe()
expected = {record.record_id for record in snapshot.records}
found = {row["id"] for row in first}
if found != expected or first != second or first_snapshot != second_snapshot:
    raise SystemExit("KaveonDB dataset rebuild coverage is incomplete or unstable")
canonical = lambda value: json.dumps(value, sort_keys=True,
    separators=(",", ":"), ensure_ascii=False).encode()
probe_sha = hashlib.sha256(canonical(first)).hexdigest()
revision_sha = hashlib.sha256(canonical([
    {"id": row["id"], "generation": row["generation"]} for row in first])).hexdigest()
result = {"target_snapshot_id": first_snapshot,
          "dataset_revision_sha256": revision_sha,
          "active_datasets": len(expected), "covered_datasets": len(found),
          "failed_datasets": 0, "first_probe_sha256": probe_sha,
          "repeat_probe_sha256": probe_sha}
if args.output.exists():
    raise SystemExit("refusing to overwrite context-cache rebuild observation")
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n")
print(json.dumps({"passed": True, "active_datasets": len(expected),
                  "target_snapshot_id": first_snapshot}, sort_keys=True))
