"""Runtime activation contract for PostgreSQL-free API operation."""

import json
import os
from datetime import datetime, timezone
from pathlib import Path

from services import engine_bridge, postgresql_retirement_gate as gate, product_store


MODE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_MODE"
EVIDENCE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_EVIDENCE"
AUDIT_KEY = "KAVEON_POSTGRESQL_RETIREMENT_AUDIT"
AUTHORITY_KEY = "KAVEONDB_AUTHORITY_FAMILIES"
MAX_AGE_KEY = "KAVEON_POSTGRESQL_RETIREMENT_MAX_AGE_HOURS"


def requested() -> bool:
    return os.getenv(MODE_KEY, "").strip().lower() == "true"


def _authority_families() -> set[str]:
    return {
        item.strip().lower()
        for item in os.getenv(AUTHORITY_KEY, "").split(",")
        if item.strip()
    }


def validate(*, now: datetime | None = None) -> dict:
    """Return verified activation metadata or reject incomplete configuration."""
    if not requested():
        return {"enabled": False, "authority": "postgresql"}
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
    evidence_path = Path(os.getenv(EVIDENCE_KEY, ""))
    audit_path = Path(os.getenv(AUDIT_KEY, ""))
    if (not evidence_path.is_file() or evidence_path.stat().st_size > gate.MAX_EVIDENCE_BYTES
            or not audit_path.is_file() or audit_path.stat().st_size > gate.MAX_EVIDENCE_BYTES):
        raise RuntimeError("PostgreSQL retirement evidence or audit is missing or oversized")
    try:
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        archived_audit = json.loads(audit_path.read_text(encoding="utf-8"))
        max_age = int(os.getenv(MAX_AGE_KEY, "24"))
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
    audit = gate.evaluate(
        evidence, now=now or datetime.now(timezone.utc), max_age_hours=max_age,
    )
    # Validate endpoint policy, service credential and configured CA before the
    # process drops its PostgreSQL configuration.
    engine_bridge._endpoint()
    engine_bridge._verify_context()
    if not os.getenv("KAVEON_ENGINE_BRIDGE_TOKEN"):
        raise RuntimeError("KaveonDB bridge credential is not configured")
    return {
        "enabled": True,
        "authority": "kaveondb",
        "authority_family_count": audit["authority_family_count"],
        "evidence_sha256": audit["evidence_sha256"],
        "checked_at": audit["checked_at"],
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
