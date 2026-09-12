"""Checkpointed operator workflow for dataset backfill and reconciliation."""

import json
import hashlib
import os
import tempfile
from pathlib import Path

from services import product_backfill


CHECKPOINT_VERSION = 1
MAX_CHECKPOINT_BYTES = product_backfill.MAX_SNAPSHOT_BYTES + 16 * 1024 * 1024


def _checkpoint_document(snapshot, next_index: int, complete: bool) -> dict:
    document = {
        "version": CHECKPOINT_VERSION,
        "family": "datasets",
        "source_watermark": snapshot.source_watermark,
        "snapshot_sha256": snapshot.snapshot_sha256,
        "next_index": next_index,
        "complete": complete,
        "records": [
            {
                "record_id": record.record_id,
                "owner_principal": record.owner_principal,
                "document": record.document,
                "payload_sha256": record.payload_sha256,
            }
            for record in snapshot.records
        ],
    }
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    document["checkpoint_sha256"] = hashlib.sha256(encoded.encode("utf-8")).hexdigest()
    return document


def save_checkpoint(path: Path, snapshot, next_index: int, *, complete: bool = False) -> None:
    product_backfill.validate_snapshot(snapshot)
    if not 0 <= next_index <= len(snapshot.records):
        raise RuntimeError("Dataset checkpoint position is invalid")
    if complete and next_index != len(snapshot.records):
        raise RuntimeError("A complete dataset checkpoint must include every record")
    document = _checkpoint_document(snapshot, next_index, complete)
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_CHECKPOINT_BYTES:
        raise RuntimeError("Dataset checkpoint exceeds its byte bound")
    destination = path.resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    backup_temporary = None
    try:
        # Keep the last complete checkpoint beside the active one.  The backup
        # is written and fsynced before replacing the active file, so a process
        # or host restart cannot turn an otherwise resumable migration into an
        # unrecoverable gap.  It is recovery evidence, not a second source of
        # truth: a corrupt active checkpoint remains fail-closed.
        if destination.exists():
            backup = destination.with_name(destination.name + ".bak")
            with tempfile.NamedTemporaryFile(
                mode="wb", dir=destination.parent, prefix=backup.name + ".", delete=False
            ) as handle:
                backup_temporary = Path(handle.name)
                os.chmod(backup_temporary, 0o600)
                handle.write(destination.read_bytes())
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(backup_temporary, backup)
            backup_temporary = None
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=destination.parent, prefix=destination.name + ".", delete=False
        ) as handle:
            temporary = Path(handle.name)
            os.chmod(temporary, 0o600)
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, destination)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()
        if backup_temporary and backup_temporary.exists():
            backup_temporary.unlink()


def load_checkpoint(path: Path):
    source = path.resolve()
    if not source.exists():
        backup = source.with_name(source.name + ".bak")
        if backup.exists():
            source = backup
        else:
            raise RuntimeError("Dataset checkpoint is missing")
    if source.stat().st_size > MAX_CHECKPOINT_BYTES:
        raise RuntimeError("Dataset checkpoint exceeds its byte bound")
    try:
        raw = json.loads(source.read_text(encoding="utf-8"))
        checkpoint_sha256 = raw.pop("checkpoint_sha256", None)
        canonical = json.dumps(raw, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        if hashlib.sha256(canonical.encode("utf-8")).hexdigest() != checkpoint_sha256:
            raise RuntimeError("Dataset checkpoint identity mismatch")
        if raw.get("version") != CHECKPOINT_VERSION or raw.get("family") != "datasets":
            raise RuntimeError("Dataset checkpoint version or family is invalid")
        records = tuple(
            product_backfill.SnapshotRecord(
                str(record["record_id"]),
                str(record["owner_principal"]),
                record["document"],
                str(record["payload_sha256"]),
            )
            for record in raw["records"]
        )
        snapshot = product_backfill.DatasetSnapshot(
            int(raw["source_watermark"]), records, str(raw["snapshot_sha256"])
        )
        product_backfill.validate_snapshot(snapshot)
        next_index = int(raw["next_index"])
        if not 0 <= next_index <= len(records):
            raise RuntimeError("Dataset checkpoint position is invalid")
        if raw.get("complete") and next_index != len(records):
            raise RuntimeError("Complete dataset checkpoint position is invalid")
        return snapshot, next_index, bool(raw.get("complete"))
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise RuntimeError("Dataset checkpoint is invalid") from error


def run(checkpoint: Path, *, apply: bool, resume: bool) -> dict:
    """Capture or resume an exact snapshot; target writes require two controls."""
    if apply and os.getenv("KAVEON_PRODUCT_MIGRATION_ENABLED") != "true":
        raise RuntimeError("Apply requires KAVEON_PRODUCT_MIGRATION_ENABLED=true")
    if resume:
        if not checkpoint.exists():
            raise RuntimeError("Resume requires an existing checkpoint")
        snapshot, next_index, complete = load_checkpoint(checkpoint)
    else:
        if checkpoint.exists():
            raise RuntimeError("Checkpoint already exists; use --resume or a new path")
        snapshot = product_backfill.capture_dataset_snapshot()
        next_index, complete = 0, False
        save_checkpoint(checkpoint, snapshot, next_index)

    if not apply:
        return {
            "mode": "dry-run",
            "source_watermark": snapshot.source_watermark,
            "source_count": len(snapshot.records),
            "snapshot_sha256": snapshot.snapshot_sha256,
            "next_index": next_index,
            "complete": complete,
        }
    created = already_present = 0
    for index in range(next_index, len(snapshot.records)):
        record = snapshot.records[index]
        single = product_backfill.DatasetSnapshot(
            snapshot.source_watermark,
            (record,),
            product_backfill.snapshot_digest((record,)),
        )
        report = product_backfill.apply_and_reconcile(single)
        created += report["created"]
        already_present += report["already_present"]
        save_checkpoint(checkpoint, snapshot, index + 1)

    final = product_backfill.apply_and_reconcile(snapshot)
    save_checkpoint(checkpoint, snapshot, len(snapshot.records), complete=True)
    return {
        **final,
        "mode": "apply",
        "created_this_run": created,
        "already_present_this_run": already_present,
        "checkpoint_complete": True,
    }
