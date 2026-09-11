"""Credential-free, fail-closed PostgreSQL authority parity gate."""

import hashlib
import json
from datetime import datetime, timezone


SCHEMA_VERSION = 1
MAX_EVIDENCE_BYTES = 4 * 1024 * 1024
REQUIRED_CHECKS = ("counts", "stable_ids", "ownership", "references", "content_hashes")
EVIDENCE_KEYS = frozenset(("schema_version", "families"))
FAMILY_KEYS = frozenset((
    "family", "tables", "status", "reconciled_at", "source_watermark",
    "source_count", "target_count", "checks", "provenance", "report_sha256",
))
PROVENANCE_KEYS = frozenset(("producer", "source_snapshot", "target_snapshot"))
AUTHORITY_FAMILIES = {
    "catalog_sources": ("catalog_sources",),
    "data_sources": ("data_sources",),
    "datasets": ("datasets",),
    "dataset_semantics": ("dataset_dimensions", "dataset_columns", "dataset_metrics"),
    "charts": ("charts",),
    "dashboards": ("dashboards",),
    "favorites": ("favorites",),
    "saved_queries": ("saved_queries",),
    "user_themes": ("user_themes",),
    "user_recents": ("user_recents",),
    "query_history": ("query_history",),
    "activity": ("activity",),
    "context_cache": ("context_snapshots", "context_answer_cache"),
    "dlm_generation": ("dlm_artifact", "dlm_value_index", "dlm_router", "dlm_answers", "dlm_sketch"),
    "chat_history": ("chat_sessions", "chat_messages"),
}
_FORBIDDEN_KEY_PARTS = ("password", "secret", "token", "credential", "connection_string", "api_key")


def _canonical(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _parse_utc(value: object) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        raise RuntimeError("reconciled_at must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise RuntimeError("reconciled_at is invalid") from error
    if parsed.utcoffset() != timezone.utc.utcoffset(parsed):
        raise RuntimeError("reconciled_at must be UTC")
    return parsed


def _reject_sensitive_keys(value: object) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).lower()
            if any(part in normalized for part in _FORBIDDEN_KEY_PARTS):
                raise RuntimeError(f"retirement evidence contains forbidden field: {key}")
            _reject_sensitive_keys(child)
    elif isinstance(value, list):
        for child in value:
            _reject_sensitive_keys(child)


def evaluate(evidence: dict, *, now: datetime, max_age_hours: int) -> dict:
    """Validate complete reconciliation evidence and return a signed audit result."""
    encoded = _canonical(evidence)
    if len(encoded) > MAX_EVIDENCE_BYTES:
        raise RuntimeError("retirement evidence exceeds its byte bound")
    if max_age_hours <= 0:
        raise RuntimeError("max_age_hours must be positive")
    if now.tzinfo is None or now.utcoffset() is None:
        raise RuntimeError("now must be timezone-aware")
    _reject_sensitive_keys(evidence)
    if set(evidence) != EVIDENCE_KEYS:
        raise RuntimeError("retirement evidence has unexpected fields")
    if evidence.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("retirement evidence schema version is invalid")
    entries = evidence.get("families")
    if not isinstance(entries, list):
        raise RuntimeError("retirement evidence families must be a list")
    by_family = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("family"), str):
            raise RuntimeError("retirement evidence family entry is invalid")
        family = entry["family"]
        if set(entry) != FAMILY_KEYS:
            raise RuntimeError(f"{family} evidence has unexpected fields")
        if family in by_family:
            raise RuntimeError(f"duplicate retirement evidence for {family}")
        by_family[family] = entry
    expected, actual = set(AUTHORITY_FAMILIES), set(by_family)
    if missing := sorted(expected - actual):
        raise RuntimeError("missing PostgreSQL authority evidence: " + ", ".join(missing))
    if unknown := sorted(actual - expected):
        raise RuntimeError("unknown PostgreSQL authority evidence: " + ", ".join(unknown))

    checked_at = now.astimezone(timezone.utc)
    normalized = []
    for family in sorted(AUTHORITY_FAMILIES):
        entry = by_family[family]
        if entry.get("tables") != list(AUTHORITY_FAMILIES[family]):
            raise RuntimeError(f"{family} table coverage does not match the authority inventory")
        reconciled_at = _parse_utc(entry.get("reconciled_at"))
        age_seconds = (checked_at - reconciled_at).total_seconds()
        if age_seconds < 0 or age_seconds > max_age_hours * 3600:
            raise RuntimeError(f"{family} reconciliation evidence is not fresh")
        if entry.get("status") != "passed":
            raise RuntimeError(f"{family} reconciliation did not pass")
        checks = entry.get("checks")
        if not isinstance(checks, dict) or set(checks) != set(REQUIRED_CHECKS):
            raise RuntimeError(f"{family} reconciliation check coverage is incomplete")
        if any(checks[name] is not True for name in REQUIRED_CHECKS):
            raise RuntimeError(f"{family} reconciliation contains a failed check")
        provenance = entry.get("provenance")
        if not isinstance(provenance, dict) or set(provenance) != PROVENANCE_KEYS:
            raise RuntimeError(f"{family} reconciliation provenance is incomplete")
        if any(not isinstance(provenance[name], str) or not 1 <= len(provenance[name]) <= 256
               for name in PROVENANCE_KEYS):
            raise RuntimeError(f"{family} reconciliation provenance is invalid")
        for field in ("source_watermark", "source_count", "target_count"):
            if type(entry.get(field)) is not int or entry[field] < 0:
                raise RuntimeError(f"{family} {field} is invalid")
        if entry["source_count"] != entry["target_count"]:
            raise RuntimeError(f"{family} source and target counts differ")
        digest = entry.get("report_sha256")
        if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise RuntimeError(f"{family} report_sha256 is invalid")
        normalized.append(entry)

    audit = {
        "schema_version": SCHEMA_VERSION,
        "gate": "postgresql-retirement-parity",
        "passed": True,
        "checked_at": checked_at.isoformat().replace("+00:00", "Z"),
        "max_age_hours": max_age_hours,
        "authority_family_count": len(AUTHORITY_FAMILIES),
        "families": normalized,
        "evidence_sha256": hashlib.sha256(encoded).hexdigest(),
    }
    audit["audit_sha256"] = hashlib.sha256(_canonical(audit)).hexdigest()
    return audit
