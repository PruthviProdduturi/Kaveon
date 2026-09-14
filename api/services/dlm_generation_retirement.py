"""Verify DLM compiled-artifact migration and legacy generated-state retirement."""

import hashlib
import json
import os
from datetime import datetime, timezone

from services import dlm_migration_evidence
from services import postgresql_evidence_collector as collector
from services import postgresql_retirement_gate as gate

SCHEMA_VERSION = 1
MAX_EVIDENCE_BYTES = 256 * 1024
MAX_ROWS = 10_000_000_000
TABLES = gate.AUTHORITY_FAMILIES["dlm_generation"]
HEX = frozenset("0123456789abcdef")


def _canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _digest(value): return hashlib.sha256(_canonical(value)).hexdigest()


def _utc(value, label):
    if not isinstance(value, str) or not value.endswith("Z"):
        raise RuntimeError(f"DLM {label} timestamp is invalid")
    try: return datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error: raise RuntimeError(f"DLM {label} timestamp is invalid") from error


def _counts(value, label):
    if not isinstance(value, dict) or set(value) != set(TABLES):
        raise RuntimeError(f"DLM {label} table coverage is incomplete")
    if any(type(count) is not int or not 0 <= count <= MAX_ROWS for count in value.values()):
        raise RuntimeError(f"DLM {label} row count is invalid")
    return value


def build_report(bundle: dict, retirement: dict, *, now: datetime, max_age_hours: int) -> dict:
    if os.getenv("KAVEON_DLM_GENERATION_RETIREMENT_ENABLED") != "true":
        raise RuntimeError("DLM generation retirement requires explicit enablement")
    if len(_canonical(retirement)) > MAX_EVIDENCE_BYTES or max_age_hours <= 0:
        raise RuntimeError("DLM generation retirement evidence is invalid")
    verified = dlm_migration_evidence.verify(bundle, now=now, max_age_hours=max_age_hours)
    expected = {"schema_version", "observed_at", "source", "deletion"}
    if not isinstance(retirement, dict) or set(retirement) != expected or retirement.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("DLM generation retirement evidence schema is invalid")
    observed = _utc(retirement["observed_at"], "observation")
    age = (now.astimezone(timezone.utc) - observed.astimezone(timezone.utc)).total_seconds()
    if age < 0 or age > max_age_hours * 3600: raise RuntimeError("DLM generation retirement evidence is stale")
    source = retirement["source"]
    if not isinstance(source, dict) or set(source) != {"snapshot_id", "watermark", "rows", "schema_sha256"}:
        raise RuntimeError("DLM generation source observation is incomplete")
    if not isinstance(source["snapshot_id"], str) or not source["snapshot_id"]:
        raise RuntimeError("DLM generation source snapshot is invalid")
    if type(source["watermark"]) is not int or source["watermark"] < 0:
        raise RuntimeError("DLM generation source watermark is invalid")
    rows = _counts(source["rows"], "source")
    schemas = source["schema_sha256"]
    if (not isinstance(schemas, dict) or set(schemas) != set(TABLES)
            or any(not isinstance(value, str) or len(value) != 64 or set(value) - HEX for value in schemas.values())):
        raise RuntimeError("DLM generation source schema identity is incomplete")
    deletion = retirement["deletion"]
    required = {"writes_fenced", "deleted_rows", "remaining_rows", "verified_at", "target_snapshot_id"}
    if not isinstance(deletion, dict) or set(deletion) != required:
        raise RuntimeError("DLM generation deletion observation is incomplete")
    deleted, remaining = _counts(deletion["deleted_rows"], "deleted"), _counts(deletion["remaining_rows"], "remaining")
    if deletion["writes_fenced"] is not True: raise RuntimeError("DLM generation writes are not fenced")
    if deleted != rows: raise RuntimeError("DLM generation deleted counts do not match source")
    if any(remaining.values()): raise RuntimeError("DLM generation PostgreSQL rows remain")
    verified_at = _utc(deletion["verified_at"], "deletion verification")
    verification_age = (now.astimezone(timezone.utc) - verified_at.astimezone(timezone.utc)).total_seconds()
    if verified_at < observed or verification_age < 0 or verification_age > max_age_hours * 3600:
        raise RuntimeError("DLM generation deletion verification is stale")
    if deletion["target_snapshot_id"] != verified["target_snapshot_id"]:
        raise RuntimeError("DLM generation target snapshot does not match compiled-artifact evidence")
    decision = _digest({"bundle_sha256": verified["bundle_sha256"], "retirement": retirement})
    report = {"schema_version": collector.REPORT_SCHEMA_VERSION, "family": "dlm_generation",
              "tables": list(TABLES), "status": "passed", "reconciled_at": deletion["verified_at"],
              "source_watermark": source["watermark"], "source_count": 0, "target_count": 0,
              "checks": {name: True for name in gate.REQUIRED_CHECKS},
              "provenance": {"producer": "dlm-generation-retirement-v1",
                             "source_snapshot": f"postgresql-retired:{decision}",
                             "target_snapshot": f"kaveondb-compiled:{verified['target_snapshot_id']}"}}
    report["report_sha256"] = hashlib.sha256(collector._canonical(report)).hexdigest()
    return report
