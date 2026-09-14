"""Runtime activation contract for PostgreSQL-free API operation."""

import json
import os
from datetime import datetime, timezone
from pathlib import Path

from services import engine_bridge, postgresql_retirement_gate as gate, product_store


MODE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_MODE"
REHEARSAL_MODE_KEY = "KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE"
EVIDENCE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_EVIDENCE"
AUDIT_KEY = "KAVEON_POSTGRESQL_RETIREMENT_AUDIT"
REPORT_DIRECTORY_KEY = "KAVEON_POSTGRESQL_RECONCILIATION_REPORTS"
RECEIPT_DIRECTORY_KEY = "KAVEON_POSTGRESQL_OPERATIONAL_RECEIPTS"
AUTHORITY_KEY = "KAVEONDB_AUTHORITY_FAMILIES"
MAX_AGE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_MAX_AGE_HOURS"


def requested() -> bool:
    return (os.getenv(MODE_KEY, "").strip().lower() == "true"
            or os.getenv(REHEARSAL_MODE_KEY, "").strip().lower() == "true")


def _final_requested() -> bool:
    return os.getenv(MODE_KEY, "").strip().lower() == "true"


def _authority_families() -> set[str]:
    return {
        item.strip().lower()
        for item in os.getenv(AUTHORITY_KEY, "").split(",")
        if item.strip()
    }


def _validate_rehearsal_evidence(now: datetime, max_age: int) -> dict:
    """Validate every prerequisite that can exist before the first PG-free boot."""
    from services import postgresql_evidence_collector as collector
    from services import postgresql_operational_evidence as operational

    reports = Path(os.getenv(REPORT_DIRECTORY_KEY, ""))
    receipts = Path(os.getenv(RECEIPT_DIRECTORY_KEY, ""))
    entries = [collector._load_report(reports / f"{family}.json", family)
               for family in sorted(gate.AUTHORITY_FAMILIES)]
    required_receipts = ("source_watermark", "outbox_drain", "write_fence",
                         "shadow_parity", "backup_identity", "durable_checkpoint")
    observed = {name: operational.load_receipt(
        receipts / f"{name}.json", name, now=now, max_age_hours=max_age,
        max_rollback_seconds=900,
    ) for name in required_receipts}
    watermark = observed["source_watermark"]["details"]["watermark"]
    if observed["outbox_drain"]["observation"]["watermark"] != watermark:
        raise RuntimeError("Restart rehearsal receipts do not share one final watermark")
    if any(entry["source_watermark"] > watermark for entry in entries):
        raise RuntimeError("Restart rehearsal family evidence is ahead of the fenced watermark")
    # Reuse the strict family validator with local placeholders only for the two
    # observations that this rehearsal exists to create later. These placeholders
    # are never emitted, archived, or accepted by final retirement mode.
    gates = {
        name: {"status": "passed", "checked_at": receipt["checked_at"],
               "evidence_id": receipt["evidence_id"], "details": receipt["details"]}
        for name, receipt in observed.items() if name in gate.GLOBAL_GATE_NAMES
    }
    timestamp = now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")
    gates["restart_recovery"] = {"status": "passed", "checked_at": timestamp,
        "evidence_id": "rehearsal-validation-placeholder", "details": {"verified": True}}
    gates["rollback"] = {"status": "passed", "checked_at": timestamp,
        "evidence_id": "rehearsal-validation-placeholder", "details": {"verified": True}}
    validated = gate.evaluate({"schema_version": gate.SCHEMA_VERSION,
        "families": entries, "gates": gates}, now=now, max_age_hours=max_age)
    return {"authority_family_count": validated["authority_family_count"],
            "evidence_sha256": validated["evidence_sha256"],
            "checked_at": validated["checked_at"], "phase": "restart_rehearsal"}


def validate(*, now: datetime | None = None) -> dict:
    """Return verified activation metadata or reject incomplete configuration."""
    if not requested():
        return {"enabled": False, "authority": "postgresql"}
    if _final_requested() and os.getenv(REHEARSAL_MODE_KEY, "").strip().lower() == "true":
        raise RuntimeError("Final retirement and restart rehearsal modes are mutually exclusive")
    configured = _authority_families()
    required = set(gate.AUTHORITY_FAMILIES)
    if configured != required:
        missing = sorted(required - configured)
        unknown = sorted(configured - required)
        detail = []
        if missing:
            detail.append("missing: " + ", ".join(missing))
        if unknown:
            detail.append("unknown: " + ", ".join(unknown))
        raise RuntimeError("KaveonDB authority-family configuration is incomplete (" + "; ".join(detail) + ")")
    from services import product_read_authority
    runtime_reads = {item.strip().lower() for item in
        os.getenv(product_read_authority.ENVIRONMENT_KEY, "").split(",") if item.strip()}
    if runtime_reads != set(product_read_authority.SUPPORTED_FAMILIES):
        raise RuntimeError("KaveonDB runtime read-authority configuration is incomplete")
    current = now or datetime.now(timezone.utc)
    try:
        max_age = int(os.getenv(MAX_AGE_KEY, "24"))
    except ValueError as error:
        raise RuntimeError("PostgreSQL retirement evidence configuration is invalid") from error
    if not _final_requested():
        audit = _validate_rehearsal_evidence(current, max_age)
    else:
        evidence_path = Path(os.getenv(EVIDENCE_KEY, ""))
        audit_path = Path(os.getenv(AUDIT_KEY, ""))
        if (not evidence_path.is_file() or evidence_path.stat().st_size > gate.MAX_EVIDENCE_BYTES
                or not audit_path.is_file() or audit_path.stat().st_size > gate.MAX_EVIDENCE_BYTES):
            raise RuntimeError("PostgreSQL retirement evidence or audit is missing or oversized")
        try:
            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            archived_audit = json.loads(audit_path.read_text(encoding="utf-8"))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            raise RuntimeError("PostgreSQL retirement evidence configuration is invalid") from error
        if not isinstance(archived_audit, dict):
            raise RuntimeError("PostgreSQL retirement audit is invalid")
        try:
            audit_time = datetime.fromisoformat(archived_audit["checked_at"].replace("Z", "+00:00"))
        except (KeyError, TypeError, ValueError) as error:
            raise RuntimeError("PostgreSQL retirement audit time is invalid") from error
        expected_audit = gate.evaluate(evidence, now=audit_time, max_age_hours=max_age)
        if archived_audit != expected_audit:
            raise RuntimeError("PostgreSQL retirement evidence does not match its audit")
        audit = gate.evaluate(evidence, now=current, max_age_hours=max_age)
    # Validate endpoint policy, service credential and configured CA before the
    # process drops its PostgreSQL configuration.
    engine_bridge._endpoint()
    engine_bridge._verify_context()
    if not os.getenv("KAVEON_ENGINE_BRIDGE_TOKEN"):
        raise RuntimeError("KaveonDB bridge credential is not configured")
    from services import dlm_compiled_artifact
    if os.getenv(dlm_compiled_artifact.LIVE_PUBLISH_KEY) != "true":
        raise RuntimeError("Live compiled DLM artifact publication is not enabled")
    if not os.getenv("KAVEON_ADLS_ACCOUNT") or not os.getenv("KAVEON_ADLS_CONTAINER"):
        raise RuntimeError("Compiled DLM artifact storage is not configured")
    return {
        "enabled": True,
        "authority": "kaveondb",
        "authority_family_count": audit["authority_family_count"],
        "evidence_sha256": audit["evidence_sha256"],
        "checked_at": audit["checked_at"],
        "phase": audit.get("phase", "final"),
    }


def probe() -> dict:
    """Probe the committed KaveonDB product authority with no source fallback."""
    state = validate()
    if not state["enabled"]:
        raise RuntimeError("PostgreSQL retirement mode is not enabled")
    # A missing sentinel is a successful authenticated point read. Any transport,
    # TLS, credential, authorization, or response-schema failure still raises.
    product_store.read("dataset", "__kaveon_health__", "kaveon-system", "Admin")
    return state
