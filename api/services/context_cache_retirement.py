"""Fail-closed evidence for retiring PostgreSQL's rebuildable DLM context cache.

Cache rows are deliberately not migrated: answer rows may contain user-derived
results and both tables are invalid once their dataset revision changes.  This
module accepts metadata-only observations from deployment-owned jobs, proves a
deterministic rebuild, and emits the normal ``context_cache`` family report only
after PostgreSQL writes are fenced and the old rows are verified absent.
"""

from __future__ import annotations

import hashlib
import json
import os
from datetime import datetime, timezone

from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate


SCHEMA_VERSION = 1
MAX_EVIDENCE_BYTES = 256 * 1024
MAX_ROWS = 10_000_000_000
HEX = frozenset("0123456789abcdef")
FORBIDDEN_KEY_PARTS = (
    "answer", "result", "question", "profile", "value", "sql", "secret",
    "token", "password", "credential", "connection_string", "api_key",
)


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _digest(value: object) -> str:
    return hashlib.sha256(_canonical(value)).hexdigest()


def _valid_digest(value: object) -> bool:
    return isinstance(value, str) and len(value) == 64 and set(value) <= HEX


def _utc(value: object) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        raise RuntimeError("context-cache timestamp must be RFC3339 UTC")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise RuntimeError("context-cache timestamp is invalid") from error
    return parsed


def _reject_payloads(value: object) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).lower()
            if any(part in normalized for part in FORBIDDEN_KEY_PARTS):
                raise RuntimeError(f"context-cache evidence contains forbidden field: {key}")
            _reject_payloads(child)
    elif isinstance(value, list):
        for child in value:
            _reject_payloads(child)


def _count(value: object, field: str) -> int:
    if type(value) is not int or not 0 <= value <= MAX_ROWS:
        raise RuntimeError(f"context-cache {field} is invalid")
    return value


def build_report(evidence: dict, *, now: datetime, max_age_hours: int) -> dict:
    """Verify a retirement observation and return an integrity-bound report.

    The source and target counts in the family report are zero because the
    authoritative post-fence state is an empty disposable cache.  Pre-deletion
    counts remain bound in ``decision_sha256`` without exposing cached content.
    """
    if os.getenv("KAVEON_CONTEXT_CACHE_RETIREMENT_ENABLED") != "true":
        raise RuntimeError("context-cache retirement requires explicit enablement")
    if not isinstance(evidence, dict) or len(_canonical(evidence)) > MAX_EVIDENCE_BYTES:
        raise RuntimeError("context-cache evidence is invalid or oversized")
    if now.tzinfo is None or now.utcoffset() is None or max_age_hours <= 0:
        raise RuntimeError("context-cache verification time parameters are invalid")
    _reject_payloads(evidence)
    expected = {"schema_version", "observed_at", "source", "rebuild", "deletion"}
    if set(evidence) != expected or evidence.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("context-cache evidence schema is invalid")
    observed = _utc(evidence["observed_at"])
    age = (now.astimezone(timezone.utc) - observed.astimezone(timezone.utc)).total_seconds()
    if age < 0 or age > max_age_hours * 3600:
        raise RuntimeError("context-cache evidence is not fresh")

    source = evidence["source"]
    source_keys = {"snapshot_id", "watermark", "snapshot_rows", "cache_rows",
                   "snapshot_schema_sha256", "cache_schema_sha256"}
    if not isinstance(source, dict) or set(source) != source_keys:
        raise RuntimeError("context-cache source observation is incomplete")
    if not isinstance(source["snapshot_id"], str) or not source["snapshot_id"]:
        raise RuntimeError("context-cache source snapshot is invalid")
    watermark = _count(source["watermark"], "source watermark")
    snapshot_rows = _count(source["snapshot_rows"], "snapshot row count")
    cache_rows = _count(source["cache_rows"], "answer-cache row count")
    if not _valid_digest(source["snapshot_schema_sha256"]) or not _valid_digest(source["cache_schema_sha256"]):
        raise RuntimeError("context-cache source schema identity is invalid")

    rebuild = evidence["rebuild"]
    rebuild_keys = {"target_snapshot_id", "dataset_revision_sha256", "active_datasets",
                    "covered_datasets", "failed_datasets", "first_probe_sha256",
                    "repeat_probe_sha256"}
    if not isinstance(rebuild, dict) or set(rebuild) != rebuild_keys:
        raise RuntimeError("context-cache rebuild observation is incomplete")
    if not isinstance(rebuild["target_snapshot_id"], str) or not rebuild["target_snapshot_id"]:
        raise RuntimeError("context-cache target snapshot is invalid")
    for field in ("dataset_revision_sha256", "first_probe_sha256", "repeat_probe_sha256"):
        if not _valid_digest(rebuild[field]):
            raise RuntimeError("context-cache rebuild identity is invalid")
    active = _count(rebuild["active_datasets"], "active dataset count")
    covered = _count(rebuild["covered_datasets"], "covered dataset count")
    failed = _count(rebuild["failed_datasets"], "failed dataset count")
    if covered != active or failed != 0:
        raise RuntimeError("context-cache rebuild coverage did not pass")
    if rebuild["first_probe_sha256"] != rebuild["repeat_probe_sha256"]:
        raise RuntimeError("context-cache rebuild is not deterministic")

    deletion = evidence["deletion"]
    deletion_keys = {"writes_fenced", "deleted_snapshot_rows", "deleted_cache_rows",
                     "remaining_snapshot_rows", "remaining_cache_rows", "verified_at"}
    if not isinstance(deletion, dict) or set(deletion) != deletion_keys:
        raise RuntimeError("context-cache deletion observation is incomplete")
    deleted_snapshots = _count(deletion["deleted_snapshot_rows"], "deleted snapshot count")
    deleted_cache = _count(deletion["deleted_cache_rows"], "deleted cache count")
    remaining_snapshots = _count(deletion["remaining_snapshot_rows"], "remaining snapshot count")
    remaining_cache = _count(deletion["remaining_cache_rows"], "remaining cache count")
    verified = _utc(deletion["verified_at"])
    if deletion["writes_fenced"] is not True:
        raise RuntimeError("context-cache writes are not fenced")
    if (deleted_snapshots, deleted_cache) != (snapshot_rows, cache_rows):
        raise RuntimeError("context-cache deletion counts do not match the source")
    if remaining_snapshots != 0 or remaining_cache != 0:
        raise RuntimeError("context-cache PostgreSQL rows remain")
    verification_age = (now.astimezone(timezone.utc) - verified.astimezone(timezone.utc)).total_seconds()
    if verified < observed or verification_age < 0 or verification_age > max_age_hours * 3600:
        raise RuntimeError("context-cache deletion verification is not fresh")

    decision_sha = _digest(evidence)
    report = {
        "schema_version": collector.REPORT_SCHEMA_VERSION,
        "family": "context_cache",
        "tables": list(gate.AUTHORITY_FAMILIES["context_cache"]),
        "status": "passed",
        "reconciled_at": deletion["verified_at"],
        "source_watermark": watermark,
        "source_count": 0,
        "target_count": 0,
        "checks": {name: True for name in gate.REQUIRED_CHECKS},
        "provenance": {
            "producer": "context-cache-retirement-v1",
            "source_snapshot": f"postgresql-retired:{decision_sha}",
            "target_snapshot": f"kaveondb-rebuilt:{rebuild['target_snapshot_id']}",
        },
    }
    report["report_sha256"] = hashlib.sha256(collector._canonical(report)).hexdigest()
    return report
