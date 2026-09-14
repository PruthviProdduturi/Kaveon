"""Verify observed PostgreSQL retirement rehearsals and assemble gate inputs."""

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

from services import postgresql_retirement_gate as retirement


SCHEMA_VERSION = 1
MAX_RECEIPT_BYTES = 256 * 1024
RECEIPT_KEYS = frozenset((
    "schema_version", "gate", "checked_at", "evidence_id", "details",
    "observation", "receipt_sha256",
))
GATES = retirement.GLOBAL_GATE_NAMES + ("durable_checkpoint",)
OBSERVATION_KEYS = {
    "source_watermark": frozenset(("source_snapshot", "watermark_observed")),
    "outbox_drain": frozenset(("query_id", "watermark", "pending_before", "pending_after")),
    "write_fence": frozenset(("deployment_revision", "readonly_probe_passed", "family_probes")),
    "shadow_parity": frozenset(("source_snapshot", "target_snapshot", "family_probes", "mismatch_count")),
    "restart_recovery": frozenset((
        "postgresql_unavailable", "api_restarted", "studio_restarted", "probe_count",
        "state_sha256_before", "state_sha256_after",
    )),
    "rollback": frozenset((
        "cutover_revision", "target_writes_fenced", "source_reads_restored",
        "source_writes_restored", "state_sha256_before", "state_sha256_after",
        "duration_seconds",
    )),
    "backup_identity": frozenset((
        "backup_id", "backup_sha256", "restore_job_id", "source_inventory_sha256",
        "restored_inventory_sha256", "restored_table_count",
    )),
    "durable_checkpoint": frozenset((
        "checkpoint_sha256_before", "checkpoint_sha256_after", "pod_uid_before",
        "pod_uid_after", "next_index_before", "next_index_after", "resume_completed",
    )),
}
_FORBIDDEN = ("password", "secret", "token", "credential", "connection_string", "api_key")


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _digest(value, label):
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        raise RuntimeError(f"{label} is not a lowercase SHA-256 digest")
    return value


def _positive_int(value, label, *, allow_zero=False):
    if type(value) is not int or value < (0 if allow_zero else 1):
        raise RuntimeError(f"{label} is invalid")
    return value


def _reject_sensitive(value):
    if isinstance(value, dict):
        for key, child in value.items():
            if any(part in str(key).lower() for part in _FORBIDDEN):
                raise RuntimeError(f"operational evidence contains forbidden field: {key}")
            _reject_sensitive(child)
    elif isinstance(value, list):
        for child in value:
            _reject_sensitive(child)


def _family_probes(value, gate):
    if not isinstance(value, list):
        raise RuntimeError(f"{gate} family probes must be a list")
    names = []
    for probe in value:
        if not isinstance(probe, dict) or set(probe) != {"family", "passed"} or probe.get("passed") is not True:
            raise RuntimeError(f"{gate} contains an incomplete family probe")
        names.append(probe.get("family"))
    if len(names) != len(set(names)) or set(names) != set(retirement.AUTHORITY_FAMILIES):
        raise RuntimeError(f"{gate} does not cover all authority families exactly once")


def _validate_observation(gate, value, details, *, max_rollback_seconds):
    if not isinstance(value, dict) or set(value) != OBSERVATION_KEYS[gate]:
        raise RuntimeError(f"{gate} observation schema is invalid")
    if gate == "source_watermark":
        _digest(value["source_snapshot"], "source snapshot")
        if value["watermark_observed"] != details["watermark"]:
            raise RuntimeError("source watermark observation does not match gate details")
    elif gate == "outbox_drain":
        _positive_int(value["query_id"], "outbox query ID")
        _positive_int(value["watermark"], "outbox watermark", allow_zero=True)
        _positive_int(value["pending_before"], "pending-before count", allow_zero=True)
        if value["pending_after"] != 0 or details["pending_events"] != 0:
            raise RuntimeError("outbox observation is not drained")
    elif gate == "write_fence":
        if not isinstance(value["deployment_revision"], str) or not value["deployment_revision"]:
            raise RuntimeError("write-fence deployment revision is missing")
        if value["readonly_probe_passed"] is not True or details["enabled"] is not True:
            raise RuntimeError("write-fence probes did not pass")
        _family_probes(value["family_probes"], gate)
    elif gate == "shadow_parity":
        _digest(value["source_snapshot"], "shadow source snapshot")
        _digest(value["target_snapshot"], "shadow target snapshot")
        _family_probes(value["family_probes"], gate)
        if value["mismatch_count"] != 0 or details["matched"] is not True:
            raise RuntimeError("shadow parity observation contains mismatches")
    elif gate == "restart_recovery":
        for name in ("postgresql_unavailable", "api_restarted", "studio_restarted"):
            if value[name] is not True:
                raise RuntimeError("restart rehearsal did not prove PostgreSQL-independent recovery")
        _positive_int(value["probe_count"], "restart probe count")
        before = _digest(value["state_sha256_before"], "pre-restart state")
        after = _digest(value["state_sha256_after"], "post-restart state")
        if before != after or details["verified"] is not True:
            raise RuntimeError("restart rehearsal changed committed state")
    elif gate == "rollback":
        if not isinstance(value["cutover_revision"], str) or not value["cutover_revision"]:
            raise RuntimeError("rollback cutover revision is missing")
        for name in ("target_writes_fenced", "source_reads_restored", "source_writes_restored"):
            if value[name] is not True:
                raise RuntimeError("rollback rehearsal did not restore safe authority")
        before = _digest(value["state_sha256_before"], "pre-cutover state")
        after = _digest(value["state_sha256_after"], "post-rollback state")
        duration = _positive_int(value["duration_seconds"], "rollback duration", allow_zero=True)
        if before != after or duration > max_rollback_seconds or details["verified"] is not True:
            raise RuntimeError("rollback rehearsal exceeded its recovery bound or changed state")
    elif gate == "backup_identity":
        if not isinstance(value["restore_job_id"], str) or not value["restore_job_id"]:
            raise RuntimeError("backup restore job ID is missing")
        source = _digest(value["source_inventory_sha256"], "source inventory")
        restored = _digest(value["restored_inventory_sha256"], "restored inventory")
        backup = _digest(value["backup_sha256"], "backup")
        if value["backup_id"] != details["backup_id"] or backup != details["backup_sha256"]:
            raise RuntimeError("backup observation does not match gate details")
        _positive_int(value["restored_table_count"], "restored table count")
        if source != restored or details["restore_verified"] is not True or not details["backup_id"]:
            raise RuntimeError("backup restore inventory did not reconcile")
    else:
        before = _digest(value["checkpoint_sha256_before"], "pre-restart checkpoint")
        after = _digest(value["checkpoint_sha256_after"], "post-restart checkpoint")
        if before != after or value["pod_uid_before"] == value["pod_uid_after"]:
            raise RuntimeError("durable checkpoint did not survive pod replacement")
        before_index = _positive_int(value["next_index_before"], "checkpoint index", allow_zero=True)
        after_index = _positive_int(value["next_index_after"], "resumed checkpoint index", allow_zero=True)
        if after_index < before_index or value["resume_completed"] is not True:
            raise RuntimeError("durable checkpoint resume did not complete")


def load_receipt(path: Path, gate: str, *, now: datetime, max_age_hours: int,
                 max_rollback_seconds: int) -> dict:
    if not path.is_file() or path.stat().st_size > MAX_RECEIPT_BYTES:
        raise RuntimeError(f"missing or oversized operational receipt for {gate}")
    try:
        receipt = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError(f"invalid operational receipt for {gate}") from error
    _reject_sensitive(receipt)
    if not isinstance(receipt, dict) or set(receipt) != RECEIPT_KEYS:
        raise RuntimeError(f"unexpected operational receipt schema for {gate}")
    if receipt["schema_version"] != SCHEMA_VERSION or receipt["gate"] != gate:
        raise RuntimeError(f"operational receipt identity mismatch for {gate}")
    claimed = receipt["receipt_sha256"]
    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    if claimed != hashlib.sha256(_canonical(unsigned)).hexdigest():
        raise RuntimeError(f"operational receipt digest mismatch for {gate}")
    checked_at = retirement._parse_utc(receipt["checked_at"])
    if now.tzinfo is None or now.utcoffset() is None or max_age_hours <= 0:
        raise RuntimeError("operational evidence freshness bounds are invalid")
    age_seconds = (now.astimezone(timezone.utc) - checked_at.astimezone(timezone.utc)).total_seconds()
    if age_seconds < 0 or age_seconds > max_age_hours * 3600:
        raise RuntimeError(f"operational receipt is not fresh for {gate}")
    if not isinstance(receipt["evidence_id"], str) or not receipt["evidence_id"]:
        raise RuntimeError(f"operational evidence ID is missing for {gate}")
    expected_details = retirement.GATE_DETAIL_KEYS.get(gate, frozenset(("verified",)))
    if not isinstance(receipt["details"], dict) or set(receipt["details"]) != expected_details:
        raise RuntimeError(f"operational gate details are invalid for {gate}")
    _validate_observation(gate, receipt["observation"], receipt["details"],
                          max_rollback_seconds=max_rollback_seconds)
    return receipt


def receipt_from_observation(gate: str, observation: dict, *, checked_at: str,
                             evidence_id: str, max_rollback_seconds: int = 900) -> dict:
    """Derive and sign a receipt from one successful structured probe result."""
    if gate == "source_watermark":
        details = {"watermark": observation.get("watermark_observed")}
    elif gate == "outbox_drain":
        details = {"pending_events": observation.get("pending_after")}
    elif gate == "write_fence":
        details = {"enabled": True}
    elif gate == "shadow_parity":
        details = {"matched": observation.get("mismatch_count") == 0}
    elif gate in {"restart_recovery", "rollback", "durable_checkpoint"}:
        details = {"verified": True}
    elif gate == "backup_identity":
        details = {"backup_id": observation.get("backup_id"),
                   "backup_sha256": observation.get("backup_sha256"),
                   "restore_verified": True}
    else:
        raise RuntimeError(f"unknown operational gate: {gate}")
    retirement._parse_utc(checked_at)
    if not isinstance(evidence_id, str) or not 1 <= len(evidence_id) <= 256:
        raise RuntimeError("operational evidence ID is invalid")
    _reject_sensitive(observation)
    _validate_observation(gate, observation, details,
                          max_rollback_seconds=max_rollback_seconds)
    value = {"schema_version": SCHEMA_VERSION, "gate": gate,
             "checked_at": checked_at, "evidence_id": evidence_id,
             "details": details, "observation": observation}
    value["receipt_sha256"] = hashlib.sha256(_canonical(value)).hexdigest()
    return value


def collect(directory: Path, *, now: datetime, max_age_hours: int = 24,
            max_rollback_seconds: int = 900) -> tuple[dict, dict]:
    if max_rollback_seconds <= 0:
        raise RuntimeError("max rollback duration must be positive")
    receipts = {
        gate: load_receipt(directory / f"{gate}.json", gate, now=now,
                           max_age_hours=max_age_hours,
                           max_rollback_seconds=max_rollback_seconds)
        for gate in GATES
    }
    if (receipts["source_watermark"]["details"]["watermark"] !=
            receipts["outbox_drain"]["observation"]["watermark"]):
        raise RuntimeError("source watermark and outbox drain receipts are not bound to the same watermark")
    gates = {
        gate: {
            "status": "passed", "checked_at": receipt["checked_at"],
            "evidence_id": receipt["evidence_id"], "details": receipt["details"],
        }
        for gate, receipt in receipts.items() if gate in retirement.GLOBAL_GATE_NAMES
    }
    operational = {
        "schema_version": 1,
        "backup_restore": {"status": "passed", "evidence_id": receipts["backup_identity"]["evidence_id"]},
        "rollback": {"status": "passed", "evidence_id": receipts["rollback"]["evidence_id"]},
        "postgresql_unavailable_restart": {
            "status": "passed", "evidence_id": receipts["restart_recovery"]["evidence_id"]},
        "durable_checkpoint": {
            "status": "passed", "evidence_id": receipts["durable_checkpoint"]["evidence_id"]},
    }
    operational["receipt_set_sha256"] = hashlib.sha256(_canonical(receipts)).hexdigest()
    return gates, operational
