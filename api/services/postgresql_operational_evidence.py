"""Verify observed PostgreSQL retirement rehearsals and assemble gate inputs."""

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

from services import postgresql_retirement_gate as retirement
from services import postgresql_free_smoke as smoke


SCHEMA_VERSION = 2
MAX_RECEIPT_BYTES = 256 * 1024
RECEIPT_KEYS = frozenset((
    "schema_version", "gate", "checked_at", "evidence_id", "details",
    "observation", "receipt_sha256",
))
BASELINE_GATES = (
    "postgresql_baseline_identity", "baseline_restore_qualification",
    "lossless_full_migration", "pre_delete_baseline_recheck",
    "exact_post_rollback_identity",
)
GATES = retirement.GLOBAL_GATE_NAMES + ("durable_checkpoint",) + BASELINE_GATES
OBSERVATION_KEYS = {
    "source_watermark": frozenset(("source_snapshot", "watermark_observed")),
    "outbox_drain": frozenset(("query_id", "watermark", "pending_before", "pending_after")),
    "write_fence": frozenset(("deployment_revision", "readonly_probe_passed", "family_probes")),
    "shadow_parity": frozenset(("source_snapshot", "target_snapshot", "family_probes", "mismatch_count")),
    "restart_recovery": frozenset((
        "postgresql_unavailable", "api_restarted", "studio_restarted", "probe_count",
        "service_state_sha256",
        "state_sha256_before", "state_sha256_after",
        "state_record_count_before", "state_record_count_after",
    )),
    "rollback": frozenset((
        "cutover_revision", "target_writes_fenced", "source_reads_restored",
        "source_writes_restored", "state_sha256_before", "state_sha256_after",
        "duration_seconds",
        "rollback_operation_count", "rollback_operation_limit",
    )),
    "backup_identity": frozenset((
        "backup_id", "backup_sha256", "restore_job_id", "source_inventory_sha256",
        "restored_inventory_sha256", "restored_table_count",
        "immutable_prefix", "manifest_sha256", "restore_executed",
    )),
    "durable_checkpoint": frozenset((
        "checkpoint_sha256_before", "checkpoint_sha256_after", "pod_uid_before",
        "pod_uid_after", "next_index_before", "next_index_after", "resume_completed",
    )),
    "postgresql_baseline_identity": frozenset((
        "baseline_evidence_id", "baseline_sha256", "table_count",
        "dataset17_utf8_verified",
    )),
    "baseline_restore_qualification": frozenset((
        "baseline_evidence_id", "baseline_sha256", "restored_sha256",
        "table_count", "restore_job_id", "exact_match",
    )),
    "lossless_full_migration": frozenset((
        "baseline_evidence_id", "baseline_sha256", "source_sha256",
        "target_sha256", "table_count", "pending_events", "failed_events",
        "manifest_published_last",
    )),
    "pre_delete_baseline_recheck": frozenset((
        "baseline_evidence_id", "baseline_sha256", "observed_sha256",
        "table_count", "writes_fenced", "outbox_pending",
    )),
    "exact_post_rollback_identity": frozenset((
        "baseline_evidence_id", "baseline_sha256", "restored_sha256",
        "table_count", "exact_match",
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
        if value["probe_count"] != len(smoke.CHECK_NAMES):
            raise RuntimeError("restart rehearsal did not cover the complete PostgreSQL-free HTTP surface")
        _digest(value["service_state_sha256"], "post-restart service state")
        before = _digest(value["state_sha256_before"], "pre-restart state")
        after = _digest(value["state_sha256_after"], "post-restart state")
        if before != after or details["verified"] is not True:
            raise RuntimeError("restart rehearsal changed committed state")
        if value["state_record_count_before"] != value["state_record_count_after"] or type(value["state_record_count_before"]) is not int or value["state_record_count_before"] < 1:
            raise RuntimeError("restart rehearsal changed state inventory size")
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
        operations = _positive_int(value["rollback_operation_count"], "rollback operation count", allow_zero=True)
        limit = _positive_int(value["rollback_operation_limit"], "rollback operation limit")
        if limit > 10000 or operations > limit:
            raise RuntimeError("rollback rehearsal exceeded its operation bound")
    elif gate == "backup_identity":
        if not isinstance(value["restore_job_id"], str) or not value["restore_job_id"]:
            raise RuntimeError("backup restore job ID is missing")
        source = _digest(value["source_inventory_sha256"], "source inventory")
        restored = _digest(value["restored_inventory_sha256"], "restored inventory")
        backup = _digest(value["backup_sha256"], "backup")
        _digest(value["manifest_sha256"], "backup manifest")
        prefix = value["immutable_prefix"]
        if (not isinstance(prefix, str) or not prefix.startswith("https://") or
                f"/backups/{value['backup_id']}/" not in prefix.rstrip("/") + "/" or "?" in prefix or "#" in prefix):
            raise RuntimeError("backup immutable prefix is invalid")
        if value["backup_id"] != details["backup_id"] or backup != details["backup_sha256"]:
            raise RuntimeError("backup observation does not match gate details")
        _positive_int(value["restored_table_count"], "restored table count")
        if source != restored or value["restore_executed"] is not True or details["restore_verified"] is not True or not details["backup_id"]:
            raise RuntimeError("backup restore inventory did not reconcile")
    elif gate == "durable_checkpoint":
        before = _digest(value["checkpoint_sha256_before"], "pre-restart checkpoint")
        after = _digest(value["checkpoint_sha256_after"], "post-restart checkpoint")
        if before != after or value["pod_uid_before"] == value["pod_uid_after"]:
            raise RuntimeError("durable checkpoint did not survive pod replacement")
        before_index = _positive_int(value["next_index_before"], "checkpoint index", allow_zero=True)
        after_index = _positive_int(value["next_index_after"], "resumed checkpoint index", allow_zero=True)
        if after_index < before_index or value["resume_completed"] is not True:
            raise RuntimeError("durable checkpoint resume did not complete")
    else:
        baseline_id = value["baseline_evidence_id"]
        baseline = _digest(value["baseline_sha256"], "PostgreSQL baseline")
        if (not isinstance(baseline_id, str) or not 1 <= len(baseline_id) <= 256
                or details.get("verified") is not True
                or details.get("baseline_evidence_id") != baseline_id
                or details.get("baseline_sha256") != baseline):
            raise RuntimeError(f"{gate} is not bound to its PostgreSQL baseline")
        table_count = _positive_int(value["table_count"], "baseline table count")
        if table_count != 7:
            raise RuntimeError(f"{gate} must cover all seven lossless tables")
        if gate == "postgresql_baseline_identity":
            if value["dataset17_utf8_verified"] is not True:
                raise RuntimeError("PostgreSQL baseline dataset 17 UTF-8 identity is unverified")
        elif gate == "baseline_restore_qualification":
            if (not isinstance(value["restore_job_id"], str) or not value["restore_job_id"]
                    or _digest(value["restored_sha256"], "restored baseline") != baseline
                    or value["exact_match"] is not True):
                raise RuntimeError("baseline restore qualification is not exact")
        elif gate == "lossless_full_migration":
            if (_digest(value["source_sha256"], "migration source") != baseline
                    or _digest(value["target_sha256"], "migration target") != baseline
                    or value["pending_events"] != 0 or value["failed_events"] != 0
                    or value["manifest_published_last"] is not True):
                raise RuntimeError("full lossless migration is incomplete or mismatched")
        elif gate == "pre_delete_baseline_recheck":
            if (_digest(value["observed_sha256"], "pre-delete baseline") != baseline
                    or value["writes_fenced"] is not True or value["outbox_pending"] != 0):
                raise RuntimeError("pre-delete baseline identity changed or is unfenced")
        elif (_digest(value["restored_sha256"], "post-rollback baseline") != baseline
              or value["exact_match"] is not True):
            raise RuntimeError("post-rollback PostgreSQL identity is not exact")


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
    expected_details = (frozenset(("verified", "baseline_evidence_id", "baseline_sha256"))
                        if gate in BASELINE_GATES else
                        retirement.GATE_DETAIL_KEYS.get(gate, frozenset(("verified",))))
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
    elif gate in BASELINE_GATES:
        details = {"verified": True,
                   "baseline_evidence_id": observation.get("baseline_evidence_id"),
                   "baseline_sha256": observation.get("baseline_sha256")}
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
    baseline_bindings = {
        (receipts[gate]["details"]["baseline_evidence_id"],
         receipts[gate]["details"]["baseline_sha256"])
        for gate in BASELINE_GATES
    }
    if len(baseline_bindings) != 1:
        raise RuntimeError("fresh PostgreSQL baseline receipts are not bound to one identity")
    gates = {
        gate: {
            "status": "passed", "checked_at": receipt["checked_at"],
            "evidence_id": receipt["evidence_id"], "details": receipt["details"],
        }
        for gate, receipt in receipts.items() if gate in retirement.GLOBAL_GATE_NAMES
    }
    operational = {
        "schema_version": 2,
        "backup_restore": {"status": "passed", "evidence_id": receipts["backup_identity"]["evidence_id"]},
        "rollback": {"status": "passed", "evidence_id": receipts["rollback"]["evidence_id"]},
        "postgresql_unavailable_restart": {
            "status": "passed", "evidence_id": receipts["restart_recovery"]["evidence_id"]},
        "durable_checkpoint": {
            "status": "passed", "evidence_id": receipts["durable_checkpoint"]["evidence_id"]},
    }
    for gate in BASELINE_GATES:
        operational[gate] = {
            "status": "passed", "evidence_id": receipts[gate]["evidence_id"],
            "baseline_evidence_id": receipts[gate]["details"]["baseline_evidence_id"],
            "baseline_sha256": receipts[gate]["details"]["baseline_sha256"],
        }
    operational["receipt_set_sha256"] = hashlib.sha256(_canonical(receipts)).hexdigest()
    return gates, operational
