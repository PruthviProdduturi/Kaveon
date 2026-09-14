"""Deterministic PostgreSQL DLM artifact to KaveonDB run reconciliation."""

import hashlib
import json
import re
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path

from fastapi import HTTPException

import database.metadata as db
from services import dlm_compiled_artifact, product_store


MAX_DLM_RUNS = 10_000


@dataclass(frozen=True)
class RunRecord:
    record_id: str
    owner_principal: str
    document: dict
    payload_sha256: str


@dataclass(frozen=True)
class RunSnapshot:
    source_watermark: int
    definition_snapshot_id: str
    records: tuple[RunRecord, ...]
    snapshot_sha256: str


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _object(value, label: str) -> dict:
    if value in (None, ""):
        return {}
    try:
        parsed = json.loads(value) if isinstance(value, str) else value
    except json.JSONDecodeError as error:
        raise RuntimeError(f"PostgreSQL DLM artifact {label} is invalid") from error
    if not isinstance(parsed, dict):
        raise RuntimeError(f"PostgreSQL DLM artifact {label} is invalid")
    return _repair_utf8_mojibake(parsed)


def _repair_utf8_mojibake(value):
    """Repair UTF-8 bytes previously decoded as Windows-1252 at the PG boundary."""
    if isinstance(value, dict):
        return {key: _repair_utf8_mojibake(child) for key, child in value.items()}
    if isinstance(value, list):
        return [_repair_utf8_mojibake(child) for child in value]
    if not isinstance(value, str) or not any(marker in value for marker in ("Ã", "Â", "â")):
        return value
    try:
        repaired = value.encode("cp1252").decode("utf-8")
    except (UnicodeEncodeError, UnicodeDecodeError):
        return value
    markers = ("Ã", "Â", "â")
    if sum(value.count(marker) for marker in markers) <= sum(
            repaired.count(marker) for marker in markers):
        return value
    return repaired


def _payload(row: dict) -> dict:
    dataset_id, version = str(row.get("id") or ""), row.get("version")
    if not dataset_id.isdecimal() or type(version) is not int or version < 1 or row.get("status") != "ready":
        raise RuntimeError(f"PostgreSQL DLM artifact {dataset_id} is unsupported")
    return {"dataset_id": dataset_id, "version": version,
            "manifest": _object(row.get("manifest"), "manifest"),
            "stats_rollup": _object(row.get("stats_rollup"), "statistics"),
            "usage_rollup": _object(row.get("usage_rollup"), "usage metadata"),
            "source_hash": str(row.get("source_hash") or ""),
            "built_at": str(row.get("built_at") or ""), "status": "ready",
            "values_indexed": int(row.get("values_indexed") or 0)}


def _stage(root: Path, relative_path: str, content: bytes) -> None:
    destination = root.resolve() / Path(relative_path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        if destination.is_file() and destination.read_bytes() == content:
            return
        raise RuntimeError("Staged DLM artifact exists with divergent bytes")
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("wb", dir=destination.parent,
                                         prefix=destination.name + ".", delete=False) as handle:
            temporary = Path(handle.name); os.chmod(temporary, 0o600)
            handle.write(content); handle.flush(); os.fsync(handle.fileno())
        os.replace(temporary, destination)
        if os.name != "nt":
            descriptor = os.open(destination.parent, os.O_RDONLY)
            try: os.fsync(descriptor)
            finally: os.close(descriptor)
    finally:
        if temporary and temporary.exists(): temporary.unlink()


_SOURCE_SQL = """
    SELECT d.id, d.created_by, a.version, a.manifest, a.stats_rollup,
           a.usage_rollup, a.source_hash, a.built_at, a.status,
           COALESCE(v.values_indexed, 0) AS values_indexed
    FROM datasets d JOIN dlm_artifact a ON a.dataset_id = CAST(d.id AS TEXT)
    LEFT JOIN (SELECT dataset_id, COUNT(*) AS values_indexed FROM dlm_value_index
               GROUP BY dataset_id) v ON v.dataset_id = a.dataset_id
    ORDER BY d.id, a.version LIMIT @param0
"""


def snapshot_digest(records: tuple[RunRecord, ...], definition_snapshot_id: str) -> str:
    digest = hashlib.sha256()
    for value in (definition_snapshot_id, *(item for record in records for item in
                  (record.record_id, record.owner_principal, record.payload_sha256))):
        encoded = value.encode()
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def validate_snapshot(snapshot: RunSnapshot) -> None:
    if snapshot.source_watermark < 0 or len(snapshot.records) > MAX_DLM_RUNS:
        raise RuntimeError("DLM run snapshot metadata is invalid")
    if snapshot.records != tuple(sorted(snapshot.records, key=lambda item: item.record_id)):
        raise RuntimeError("DLM run snapshot order is invalid")
    for record in snapshot.records:
        document = record.document
        if hashlib.sha256(_canonical(document)).hexdigest() != record.payload_sha256:
            raise RuntimeError(f"DLM run {record.record_id} identity is invalid")
        if set(document) != {"definition_id", "definition_revision", "status", "artifact"}:
            raise RuntimeError(f"DLM run {record.record_id} document is invalid")
        artifact = document.get("artifact")
        identity = re.fullmatch(r"([0-9]+)-v([1-9][0-9]*)", record.record_id)
        expected_path = (f"dlm/{identity.group(1)}/v{identity.group(2)}/compiled.json"
                         if identity else None)
        if (document.get("status") != "ready" or not identity
                or document.get("definition_id") != identity.group(1)
                or type(document.get("definition_revision")) is not int
                or document["definition_revision"] < 1 or not isinstance(artifact, dict)
                or set(artifact) != {"path", "sha256"}
                or re.fullmatch(r"[0-9a-f]{64}", artifact.get("sha256", "")) is None
                or artifact.get("path") != expected_path):
            raise RuntimeError(f"DLM run {record.record_id} document is invalid")
    if snapshot_digest(snapshot.records, snapshot.definition_snapshot_id) != snapshot.snapshot_sha256:
        raise RuntimeError("DLM run snapshot identity mismatch")


def capture_snapshot(artifact_root: Path) -> RunSnapshot:
    """Capture only ready legacy artifacts whose exact canonical bytes are staged locally."""
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        watermark = transaction.query_one(
            "SELECT COALESCE(MAX(source_sequence), 0) AS watermark FROM product_migration_outbox"
        ) or {}
        rows = transaction.query(_SOURCE_SQL, [MAX_DLM_RUNS + 1])["rows"]
    if len(rows) > MAX_DLM_RUNS:
        raise RuntimeError("DLM run snapshot exceeds its record bound")
    records, snapshot_id = [], None
    for row in rows:
        dataset_id, owner = str(row["id"]), str(row["created_by"])
        compiled = _payload(row); version = compiled["version"]
        artifact_bytes = dlm_compiled_artifact._canonical(compiled)
        relative_path = f"dlm/{dataset_id}/v{version}/compiled.json"
        _stage(artifact_root, relative_path, artifact_bytes)
        definition = product_store.read("dlm_definition", dataset_id, owner, "Admin")
        if definition is None:
            raise RuntimeError(f"KaveonDB DLM definition {dataset_id} is missing")
        current_snapshot, revision = str(definition.get("snapshot_id") or ""), definition.get("revision")
        if (not current_snapshot or (snapshot_id is not None and snapshot_id != current_snapshot)
                or type(revision) is not int or revision < 1):
            raise RuntimeError("KaveonDB DLM definition snapshot or revision is invalid")
        snapshot_id = current_snapshot
        record_id = f"{dataset_id}-v{version}"
        document = {"definition_id": dataset_id, "definition_revision": revision, "status": "ready",
                    "artifact": {"path": relative_path,
                                 "sha256": hashlib.sha256(artifact_bytes).hexdigest()}}
        records.append(RunRecord(record_id, owner, document,
                                 hashlib.sha256(_canonical(document)).hexdigest()))
    immutable = tuple(records)
    snapshot_id = snapshot_id or "empty"
    return RunSnapshot(int(watermark.get("watermark") or 0), snapshot_id, immutable,
                       snapshot_digest(immutable, snapshot_id))


def restage_artifacts(snapshot: RunSnapshot, artifact_root: Path) -> None:
    """Recreate ephemeral staging after pod replacement from unchanged source rows."""
    validate_snapshot(snapshot)
    with db.transaction() as transaction:
        transaction.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        rows = transaction.query(_SOURCE_SQL, [MAX_DLM_RUNS + 1])["rows"]
    expected = {record.record_id: record for record in snapshot.records}
    if len(rows) != len(expected):
        raise RuntimeError("PostgreSQL DLM artifact set changed after checkpoint")
    seen = set()
    for row in rows:
        compiled = _payload(row); dataset_id = compiled["dataset_id"]
        record_id = f"{dataset_id}-v{compiled['version']}"; record = expected.get(record_id)
        content = dlm_compiled_artifact._canonical(compiled)
        path = f"dlm/{dataset_id}/v{compiled['version']}/compiled.json"
        if (record is None or record.document["artifact"]["path"] != path
                or record.document["artifact"]["sha256"] != hashlib.sha256(content).hexdigest()):
            raise RuntimeError("PostgreSQL DLM artifact changed after checkpoint")
        _stage(artifact_root, path, content); seen.add(record_id)
    if seen != set(expected):
        raise RuntimeError("PostgreSQL DLM artifact checkpoint coverage changed")


def apply_and_reconcile(snapshot: RunSnapshot) -> dict:
    validate_snapshot(snapshot)
    created = already_present = 0
    for record in snapshot.records:
        target = product_store.read("dlm_run", record.record_id, record.owner_principal, "Admin")
        if target is not None and target.get("document") == record.document:
            already_present += 1
            continue
        building = {**record.document, "status": "building", "artifact": None}
        if target is None:
            try:
                product_store.transact([
                    product_store.ProductMutation("create", "dlm_run", record.record_id, building),
                ], record.owner_principal, "Admin")
                target = {"document": building, "revision": 1}
            except HTTPException as error:
                target = product_store.read("dlm_run", record.record_id,
                                            record.owner_principal, "Admin")
                if error.status_code != 409 or target is None:
                    raise
        if target.get("document") == record.document:
            already_present += 1
            continue
        revision = target.get("revision")
        if target.get("document") != building or type(revision) is not int or revision < 1:
            raise RuntimeError(f"KaveonDB DLM run {record.record_id} diverges")
        try:
            product_store.transact([
                product_store.ProductMutation("update", "dlm_run", record.record_id,
                                              record.document, expected_revision=revision),
            ], record.owner_principal, "Admin")
        except HTTPException as error:
            resolved = product_store.read("dlm_run", record.record_id,
                                          record.owner_principal, "Admin")
            if error.status_code != 409 or resolved is None or resolved.get("document") != record.document:
                raise
        created += 1
    for record in snapshot.records:
        target = product_store.read("dlm_run", record.record_id, record.owner_principal, "Admin")
        if target is None or target.get("document") != record.document:
            raise RuntimeError(f"KaveonDB DLM run {record.record_id} failed reconciliation")
    return {"family": "dlm_runs", "source_watermark": snapshot.source_watermark,
            "definition_snapshot_id": snapshot.definition_snapshot_id,
            "source_count": len(snapshot.records), "created": created,
            "already_present": already_present, "reconciled": len(snapshot.records),
            "snapshot_sha256": snapshot.snapshot_sha256}
