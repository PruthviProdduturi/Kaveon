"""Checkpointed, default-dry saved-query backfill operation."""

import hashlib
import json
import os
import tempfile
from pathlib import Path

from services import saved_query_backfill as backfill

VERSION = 1
MAX_CHECKPOINT_BYTES = 16 * 1024 * 1024


def _document(snapshot, next_index, complete):
    value = {
        "version": VERSION,
        "family": "saved_queries",
        "source_watermark": snapshot.source_watermark,
        "snapshot_sha256": snapshot.snapshot_sha256,
        "next_index": next_index,
        "complete": complete,
        "records": [record.__dict__ for record in snapshot.records],
    }
    value["checkpoint_sha256"] = hashlib.sha256(json.dumps(
        value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return value


def save(path: Path, snapshot, next_index: int, complete: bool = False) -> None:
    backfill.validate_snapshot(snapshot)
    if not 0 <= next_index <= len(snapshot.records) or (complete and next_index != len(snapshot.records)):
        raise RuntimeError("saved-query checkpoint position is invalid")
    encoded = json.dumps(_document(snapshot, next_index, complete), sort_keys=True,
                         separators=(",", ":")).encode()
    if len(encoded) > MAX_CHECKPOINT_BYTES:
        raise RuntimeError("saved-query checkpoint exceeds its byte bound")
    destination = path.resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="wb", dir=destination.parent,
                                         prefix=destination.name + ".", delete=False) as handle:
            temporary = Path(handle.name)
            os.chmod(temporary, 0o600)
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, destination)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def load(path: Path):
    if path.stat().st_size > MAX_CHECKPOINT_BYTES:
        raise RuntimeError("saved-query checkpoint exceeds its byte bound")
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        claimed = raw.pop("checkpoint_sha256")
        actual = hashlib.sha256(json.dumps(raw, sort_keys=True,
                                           separators=(",", ":")).encode()).hexdigest()
        if claimed != actual or raw["version"] != VERSION or raw["family"] != "saved_queries":
            raise RuntimeError("saved-query checkpoint identity is invalid")
        records = tuple(backfill.SavedQueryRecord(**record) for record in raw["records"])
        snapshot = backfill.SavedQuerySnapshot(int(raw["source_watermark"]), records,
                                               str(raw["snapshot_sha256"]))
        backfill.validate_snapshot(snapshot)
        position, complete = int(raw["next_index"]), bool(raw["complete"])
        if not 0 <= position <= len(records) or (complete and position != len(records)):
            raise RuntimeError("saved-query checkpoint position is invalid")
        return snapshot, position, complete
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise RuntimeError("saved-query checkpoint is invalid") from error


def run(checkpoint: Path, *, apply: bool, resume: bool) -> dict:
    if apply and os.getenv("KAVEON_SAVED_QUERY_MIGRATION_ENABLED") != "true":
        raise RuntimeError("Apply requires KAVEON_SAVED_QUERY_MIGRATION_ENABLED=true")
    if resume:
        if not checkpoint.exists():
            raise RuntimeError("Resume requires an existing checkpoint")
        snapshot, position, complete = load(checkpoint)
    else:
        if checkpoint.exists():
            raise RuntimeError("Checkpoint already exists; use --resume or a new path")
        snapshot, position, complete = backfill.capture_snapshot(), 0, False
        save(checkpoint, snapshot, 0)
    if not apply:
        return {"mode": "dry-run", "source_count": len(snapshot.records),
                "next_index": position, "complete": complete,
                "snapshot_sha256": snapshot.snapshot_sha256}
    for index in range(position, len(snapshot.records)):
        record = snapshot.records[index]
        single = backfill.SavedQuerySnapshot(snapshot.source_watermark, (record,),
                                             backfill.snapshot_digest((record,)))
        backfill.apply_and_reconcile(single)
        save(checkpoint, snapshot, index + 1)
    report = backfill.apply_and_reconcile(snapshot)
    save(checkpoint, snapshot, len(snapshot.records), True)
    return {**report, "mode": "apply", "checkpoint_complete": True}
