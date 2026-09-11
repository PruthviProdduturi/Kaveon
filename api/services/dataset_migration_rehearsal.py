"""Fail-closed verifier for a live dataset migration rehearsal receipt."""

import hashlib
import json
import uuid
from datetime import datetime, timezone


SCHEMA_VERSION = 1
MAX_RECEIPT_BYTES = 1024 * 1024
PARITY_CHECKS = ("counts", "stable_ids", "ownership", "references", "content_hashes")
TOP_LEVEL_KEYS = frozenset((
    "schema_version", "family", "run_id", "completed_at", "source_watermark",
    "replay", "parity", "fencing", "restart", "rollback", "receipt_sha256",
))
REPLAY_KEYS = frozenset(("first_sequence", "last_sequence", "pending_events", "failed_events"))
PARITY_KEYS = frozenset(("watermark", "source_count", "target_count", "checks", "report_sha256"))
FENCING_KEYS = frozenset(("source_writes_fenced", "fenced_watermark"))
RESTART_KEYS = frozenset(("head_before", "head_after", "parity_passed"))
ROLLBACK_KEYS = frozenset(("target_writes_fenced", "source_reads_restored", "source_writes_restored", "duration_seconds"))
_FORBIDDEN_KEY_PARTS = ("password", "secret", "token", "credential", "connection_string", "api_key")


def _canonical(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _reject_sensitive_keys(value: object) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if any(part in str(key).lower() for part in _FORBIDDEN_KEY_PARTS):
                raise RuntimeError(f"dataset rehearsal receipt contains forbidden field: {key}")
            _reject_sensitive_keys(child)
    elif isinstance(value, list):
        for child in value:
            _reject_sensitive_keys(child)


def _exact_object(value: object, keys: frozenset[str], name: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise RuntimeError(f"dataset rehearsal {name} evidence is incomplete")
    return value


def _nonnegative_int(value: object, name: str) -> int:
    if type(value) is not int or value < 0:
        raise RuntimeError(f"dataset rehearsal {name} is invalid")
    return value


def _digest(value: object, name: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        raise RuntimeError(f"dataset rehearsal {name} is invalid")
    return value


def verify(receipt: dict, *, now: datetime, max_age_hours: int, max_rollback_seconds: int) -> dict:
    """Verify replay, parity, fence, restart and rollback evidence from one run."""
    encoded = _canonical(receipt)
    if len(encoded) > MAX_RECEIPT_BYTES:
        raise RuntimeError("dataset rehearsal receipt exceeds its byte bound")
    if max_age_hours <= 0 or max_rollback_seconds <= 0:
        raise RuntimeError("dataset rehearsal verification bounds must be positive")
    if now.tzinfo is None or now.utcoffset() is None:
        raise RuntimeError("now must be timezone-aware")
    _reject_sensitive_keys(receipt)
    if set(receipt) != TOP_LEVEL_KEYS or receipt.get("schema_version") != SCHEMA_VERSION:
        raise RuntimeError("dataset rehearsal receipt schema is invalid")
    if receipt.get("family") != "datasets":
        raise RuntimeError("dataset rehearsal family must be datasets")
    try:
        uuid.UUID(str(receipt.get("run_id")))
        completed_at = datetime.fromisoformat(str(receipt["completed_at"]).removesuffix("Z") + "+00:00")
    except (ValueError, TypeError) as error:
        raise RuntimeError("dataset rehearsal identity or completion time is invalid") from error
    age = (now.astimezone(timezone.utc) - completed_at).total_seconds()
    if not str(receipt["completed_at"]).endswith("Z") or age < 0 or age > max_age_hours * 3600:
        raise RuntimeError("dataset rehearsal receipt is not fresh")

    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    if receipt["receipt_sha256"] != hashlib.sha256(_canonical(unsigned)).hexdigest():
        raise RuntimeError("dataset rehearsal receipt digest mismatch")

    watermark = _nonnegative_int(receipt["source_watermark"], "source watermark")
    replay = _exact_object(receipt["replay"], REPLAY_KEYS, "replay")
    first = _nonnegative_int(replay["first_sequence"], "first replay sequence")
    last = _nonnegative_int(replay["last_sequence"], "last replay sequence")
    pending = _nonnegative_int(replay["pending_events"], "pending replay events")
    failed = _nonnegative_int(replay["failed_events"], "failed replay events")
    if first > last or last != watermark or pending != 0 or failed != 0:
        raise RuntimeError("dataset rehearsal replay did not drain exactly through the watermark")

    parity = _exact_object(receipt["parity"], PARITY_KEYS, "parity")
    checks = parity["checks"]
    if (_nonnegative_int(parity["watermark"], "parity watermark") != watermark or
            _nonnegative_int(parity["source_count"], "source count") !=
            _nonnegative_int(parity["target_count"], "target count") or
            not isinstance(checks, dict) or set(checks) != set(PARITY_CHECKS) or
            any(checks[name] is not True for name in PARITY_CHECKS)):
        raise RuntimeError("dataset rehearsal parity did not pass at the fenced watermark")
    _digest(parity["report_sha256"], "parity report digest")

    fencing = _exact_object(receipt["fencing"], FENCING_KEYS, "fencing")
    if (fencing["source_writes_fenced"] is not True or
            _nonnegative_int(fencing["fenced_watermark"], "fenced watermark") != watermark):
        raise RuntimeError("dataset rehearsal source fence is not proven")
    restart = _exact_object(receipt["restart"], RESTART_KEYS, "restart")
    before = _digest(restart["head_before"], "pre-restart head")
    after = _digest(restart["head_after"], "post-restart head")
    if before != after or restart["parity_passed"] is not True:
        raise RuntimeError("dataset rehearsal restart changed or invalidated the committed head")
    rollback = _exact_object(receipt["rollback"], ROLLBACK_KEYS, "rollback")
    duration = _nonnegative_int(rollback["duration_seconds"], "rollback duration")
    if (rollback["target_writes_fenced"] is not True or
            rollback["source_reads_restored"] is not True or
            rollback["source_writes_restored"] is not True or
            duration > max_rollback_seconds):
        raise RuntimeError("dataset rehearsal rollback did not meet its recovery gate")

    return {
        "schema_version": SCHEMA_VERSION,
        "gate": "dataset-migration-rehearsal",
        "passed": True,
        "run_id": receipt["run_id"],
        "source_watermark": watermark,
        "checked_at": now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z"),
        "receipt_sha256": receipt["receipt_sha256"],
    }
