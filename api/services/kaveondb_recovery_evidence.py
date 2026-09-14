"""Content-free KaveonDB state and immutable backup evidence validation."""

import hashlib
import json
from urllib.parse import urlparse

MAX_RECORDS = 1_000_000
MAX_ROLLBACK_OPERATIONS = 10_000
RECORD_KEYS = {"kind", "id", "revision", "document_sha256"}
OBJECT_KEYS = {"path", "etag", "size", "sha256"}


def _canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _digest(value, label):
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        raise RuntimeError(f"{label} is not a lowercase SHA-256 digest")
    return value


def state_identity(records: list[dict]) -> dict:
    """Digest only record identities/revisions/document hashes, never documents."""
    if not isinstance(records, list) or len(records) > MAX_RECORDS:
        raise RuntimeError("KaveonDB state inventory is invalid or oversized")
    normalized = []
    for record in records:
        if (not isinstance(record, dict) or set(record) != RECORD_KEYS
                or not all(isinstance(record[key], str) and record[key] for key in ("kind", "id"))
                or type(record["revision"]) is not int or record["revision"] < 1):
            raise RuntimeError("KaveonDB state inventory record is invalid")
        _digest(record["document_sha256"], "document")
        normalized.append(record)
    normalized.sort(key=lambda item: (item["kind"], item["id"]))
    if len({(item["kind"], item["id"]) for item in normalized}) != len(normalized):
        raise RuntimeError("KaveonDB state inventory contains duplicate identities")
    return {"record_count": len(normalized), "state_sha256": hashlib.sha256(_canonical(normalized)).hexdigest()}


def validate_backup_manifest(manifest: dict) -> dict:
    expected = {"schema_version", "backup_id", "immutable_prefix", "state_sha256", "record_count", "objects"}
    if not isinstance(manifest, dict) or set(manifest) != expected or manifest["schema_version"] != 1:
        raise RuntimeError("KaveonDB backup manifest schema is invalid")
    backup_id = manifest["backup_id"]
    parsed = urlparse(manifest["immutable_prefix"] if isinstance(manifest["immutable_prefix"], str) else "")
    if (not isinstance(backup_id, str) or not backup_id or parsed.scheme != "https" or not parsed.hostname
            or not parsed.hostname.endswith((".blob.core.windows.net", ".dfs.core.windows.net"))
            or parsed.query or parsed.fragment or f"/backups/{backup_id}/" not in parsed.path.rstrip("/") + "/"):
        raise RuntimeError("KaveonDB backup prefix is not an immutable ADLS backup identity")
    _digest(manifest["state_sha256"], "backup state")
    if type(manifest["record_count"]) is not int or manifest["record_count"] < 0:
        raise RuntimeError("KaveonDB backup record count is invalid")
    objects = manifest["objects"]
    if not isinstance(objects, list) or not objects or len(objects) > MAX_RECORDS:
        raise RuntimeError("KaveonDB backup object inventory is invalid")
    paths = set()
    for item in objects:
        if (not isinstance(item, dict) or set(item) != OBJECT_KEYS or not isinstance(item["path"], str)
                or not item["path"] or item["path"].startswith(("/", "http")) or ".." in item["path"].split("/")
                or not isinstance(item["etag"], str) or not item["etag"]
                or type(item["size"]) is not int or item["size"] < 0):
            raise RuntimeError("KaveonDB backup object is invalid")
        _digest(item["sha256"], "backup object")
        if item["path"] in paths: raise RuntimeError("KaveonDB backup object path is duplicated")
        paths.add(item["path"])
    return {"backup_id": backup_id, "immutable_prefix": manifest["immutable_prefix"],
            "manifest_sha256": hashlib.sha256(_canonical(manifest)).hexdigest(),
            "object_count": len(objects), "state_sha256": manifest["state_sha256"],
            "record_count": manifest["record_count"]}


def validate_rollback_control(value: dict) -> dict:
    keys = {"cutover_revision", "expected_state_sha256", "max_operations", "max_duration_seconds"}
    if (not isinstance(value, dict) or set(value) != keys or not isinstance(value["cutover_revision"], str)
            or not value["cutover_revision"] or type(value["max_operations"]) is not int
            or not 1 <= value["max_operations"] <= MAX_ROLLBACK_OPERATIONS
            or type(value["max_duration_seconds"]) is not int or not 1 <= value["max_duration_seconds"] <= 3600):
        raise RuntimeError("KaveonDB rollback control is invalid or unbounded")
    _digest(value["expected_state_sha256"], "rollback expected state")
    return dict(value)
