"""Credential-free DLM definition/run rehearsal evidence bundle and verifier."""

import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path

from services import dlm_definition_backfill_operation as definitions
from services import dlm_run_backfill_operation as runs

SCHEMA_VERSION = 1
MAX_INPUT_BYTES = 4 * 1024 * 1024
HEX = set("0123456789abcdef")
FORBIDDEN = ("secret", "token", "password", "credential", "connection_string", "api_key")


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _load_json(path: Path):
    if not path.is_file() or path.stat().st_size > MAX_INPUT_BYTES:
        raise RuntimeError(f"missing or oversized rehearsal input: {path.name}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError(f"invalid rehearsal input: {path.name}") from error


def _digest(value) -> str:
    return hashlib.sha256(_canonical(value)).hexdigest()


def collect(definition_checkpoint: Path, run_checkpoint: Path, artifact_receipts: Path,
            definition_report: Path, run_report: Path, target_observations: Path,
            *, collected_at: datetime) -> dict:
    if os.getenv("KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED") != "true":
        raise RuntimeError("collection requires KAVEON_DLM_REHEARSAL_EVIDENCE_ENABLED=true")
    definition_snapshot, definition_position, definition_complete = definitions.load(definition_checkpoint)
    run_snapshot, run_position, run_complete = runs.load(run_checkpoint)
    if not definition_complete or definition_position != len(definition_snapshot.records):
        raise RuntimeError("DLM definition checkpoint is incomplete")
    if not run_complete or run_position != len(run_snapshot.records):
        raise RuntimeError("DLM run checkpoint is incomplete")
    definition_result, run_result = _load_json(definition_report), _load_json(run_report)
    receipts, observations = _load_json(artifact_receipts), _load_json(target_observations)
    bundle = {
        "schema_version": SCHEMA_VERSION,
        "collected_at": collected_at.astimezone(timezone.utc).isoformat().replace("+00:00", "Z"),
        "source": {
            "definition_watermark": definition_snapshot.source_watermark,
            "definition_snapshot_sha256": definition_snapshot.snapshot_sha256,
            "run_watermark": run_snapshot.source_watermark,
            "run_snapshot_sha256": run_snapshot.snapshot_sha256,
        },
        "checkpoints": {
            "definition_sha256": hashlib.sha256(definition_checkpoint.read_bytes()).hexdigest(),
            "run_sha256": hashlib.sha256(run_checkpoint.read_bytes()).hexdigest(),
        },
        "definition_capture_snapshot": definition_snapshot.dataset_snapshot_id,
        "run_capture_snapshot": run_snapshot.definition_snapshot_id,
        "definition_ids": [record.record_id for record in definition_snapshot.records],
        "runs": [{"id": record.record_id, "definition_id": record.document["definition_id"],
                  "definition_revision": record.document["definition_revision"],
                  **record.document["artifact"]} for record in run_snapshot.records],
        "artifact_receipts": receipts,
        "target_observations": observations,
        "reconciliation": {"definitions": definition_result, "runs": run_result},
    }
    verify(bundle, now=collected_at, max_age_hours=1)
    bundle["bundle_sha256"] = _digest(bundle)
    return bundle


def _valid_hash(value) -> bool:
    return isinstance(value, str) and len(value) == 64 and set(value) <= HEX


def _reject_sensitive(value):
    if isinstance(value, dict):
        for key, child in value.items():
            if any(part in str(key).lower() for part in FORBIDDEN):
                raise RuntimeError("DLM rehearsal evidence contains a forbidden field")
            _reject_sensitive(child)
    elif isinstance(value, list):
        for child in value:
            _reject_sensitive(child)


def verify(bundle: dict, *, now: datetime, max_age_hours: int) -> dict:
    if max_age_hours <= 0 or now.tzinfo is None or now.utcoffset() is None:
        raise RuntimeError("evidence verification time parameters are invalid")
    claimed = bundle.get("bundle_sha256")
    unsigned = {key: value for key, value in bundle.items() if key != "bundle_sha256"}
    if len(_canonical(bundle)) > MAX_INPUT_BYTES:
        raise RuntimeError("DLM rehearsal evidence exceeds its byte bound")
    _reject_sensitive(unsigned)
    expected_keys = {"schema_version", "collected_at", "source", "checkpoints",
                     "definition_capture_snapshot", "run_capture_snapshot", "definition_ids",
                     "runs", "artifact_receipts", "target_observations", "reconciliation"}
    if set(unsigned) != expected_keys or bundle.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("DLM rehearsal evidence schema is invalid")
    if claimed is not None and (not _valid_hash(claimed) or claimed != _digest(unsigned)):
        raise RuntimeError("DLM rehearsal evidence identity mismatch")
    try:
        collected = datetime.fromisoformat(unsigned["collected_at"].replace("Z", "+00:00"))
    except (AttributeError, ValueError) as error:
        raise RuntimeError("DLM rehearsal timestamp is invalid") from error
    age = (now.astimezone(timezone.utc) - collected.astimezone(timezone.utc)).total_seconds()
    if not unsigned["collected_at"].endswith("Z") or age < 0 or age > max_age_hours * 3600:
        raise RuntimeError("DLM rehearsal evidence is stale")
    source, checkpoints = unsigned["source"], unsigned["checkpoints"]
    if set(source) != {"definition_watermark", "definition_snapshot_sha256", "run_watermark",
                       "run_snapshot_sha256"} or set(checkpoints) != {"definition_sha256", "run_sha256"}:
        raise RuntimeError("DLM rehearsal source binding is incomplete")
    if any(type(source[key]) is not int or source[key] < 0 for key in
           ("definition_watermark", "run_watermark")) or any(not _valid_hash(value) for value in
           (source["definition_snapshot_sha256"], source["run_snapshot_sha256"], *checkpoints.values())):
        raise RuntimeError("DLM rehearsal source binding is invalid")
    run_entries = unsigned["runs"]
    run_by_id = {entry.get("id"): entry for entry in run_entries if isinstance(entry, dict)}
    definition_ids = unsigned["definition_ids"]
    if (not isinstance(definition_ids, list) or len(set(definition_ids)) != len(definition_ids)
            or any(not isinstance(value, str) or not value for value in definition_ids)):
        raise RuntimeError("DLM rehearsal definition coverage is invalid")
    if len(run_by_id) != len(run_entries) or any(
            set(entry) != {"id", "definition_id", "definition_revision", "path", "sha256"}
            or entry["definition_id"] not in definition_ids
            or type(entry["definition_revision"]) is not int or entry["definition_revision"] < 1
            or not _valid_hash(entry["sha256"])
                                                for entry in run_entries):
        raise RuntimeError("DLM rehearsal run coverage is invalid")
    receipts = unsigned["artifact_receipts"]
    receipt_by_path = {entry.get("path"): entry for entry in receipts if isinstance(entry, dict)}
    expected_paths = {entry["path"] for entry in run_entries}
    if set(receipt_by_path) != expected_paths or len(receipts) != len(receipt_by_path):
        raise RuntimeError("DLM rehearsal artifact receipt coverage is incomplete")
    for run in run_entries:
        receipt = receipt_by_path[run["path"]]
        if (set(receipt) != {"path", "sha256", "bytes", "status"}
                or receipt["sha256"] != run["sha256"] or receipt["status"] != "verified"
                or type(receipt["bytes"]) is not int or receipt["bytes"] < 1):
            raise RuntimeError("DLM rehearsal artifact receipt is invalid")
    observations = unsigned["target_observations"]
    if not isinstance(observations, dict) or set(observations) != {"snapshot_id", "definitions", "runs"}:
        raise RuntimeError("DLM rehearsal target observations are incomplete")
    if not isinstance(observations["snapshot_id"], str) or not observations["snapshot_id"]:
        raise RuntimeError("DLM rehearsal target snapshot is invalid")
    for family, expected_ids in (("definitions", set(unsigned["definition_ids"])),
                                 ("runs", set(run_by_id))):
        entries = observations[family]
        found = {entry.get("id") for entry in entries if isinstance(entry, dict)}
        if found != expected_ids or len(found) != len(entries) or any(
                set(entry) != {"id", "generation"} or type(entry["generation"]) is not int
                or entry["generation"] < 1 for entry in entries):
            raise RuntimeError(f"DLM rehearsal {family} generation coverage is invalid")
    reconciliation = unsigned["reconciliation"]
    for family, expected_count, snapshot_hash in (
            ("definitions", len(unsigned["definition_ids"]), source["definition_snapshot_sha256"]),
            ("runs", len(run_entries), source["run_snapshot_sha256"])):
        report = reconciliation.get(family) if isinstance(reconciliation, dict) else None
        if (not isinstance(report, dict) or report.get("reconciled") != expected_count
                or report.get("source_count") != expected_count
                or report.get("snapshot_sha256") != snapshot_hash):
            raise RuntimeError(f"DLM rehearsal {family} reconciliation is mismatched")
    return {"passed": True, "bundle_sha256": claimed or _digest(unsigned),
            "definition_count": len(unsigned["definition_ids"]), "run_count": len(run_entries),
            "target_snapshot_id": observations["snapshot_id"]}
