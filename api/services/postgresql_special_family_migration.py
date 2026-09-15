"""Lossless, CAS-safe publication of the seven PostgreSQL special families."""

from __future__ import annotations

import base64
import hashlib
import json
from datetime import date, datetime, timezone
from decimal import Decimal

TABLES = ("context_answer_cache", "context_snapshots", "dlm_answers",
          "dlm_artifact", "dlm_router", "dlm_sketch", "dlm_value_index")
SCHEMA_VERSION = 1
MAX_ROWS = 10_000_000
MAX_BYTES = 256 * 1024 * 1024
MAX_CAS_ATTEMPTS = 8
HEX = frozenset("0123456789abcdef")


def _value(value):
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        if value != value or value in (float("inf"), float("-inf")):
            raise RuntimeError("special-family values must be finite")
        return {"$float": value.hex()}
    if isinstance(value, Decimal):
        return {"$decimal": str(value)}
    if isinstance(value, bytes):
        return {"$bytes": base64.b64encode(value).decode("ascii")}
    if isinstance(value, datetime):
        if value.tzinfo is None or value.utcoffset() is None:
            raise RuntimeError("special-family timestamps must include a timezone")
        return {"$datetime": value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")}
    if isinstance(value, date):
        return {"$date": value.isoformat()}
    if isinstance(value, (list, tuple)):
        return [_value(item) for item in value]
    if isinstance(value, dict):
        if any(not isinstance(key, str) for key in value):
            raise RuntimeError("special-family JSON object keys must be strings")
        return {key: _value(child) for key, child in value.items()}
    raise RuntimeError(f"unsupported special-family value type: {type(value).__name__}")


def _canonical(value) -> bytes:
    return json.dumps(_value(value), sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def _sha(value) -> str:
    return hashlib.sha256(_canonical(value)).hexdigest()


def _digest(value, label="digest"):
    if not isinstance(value, str) or len(value) != 64 or set(value) - HEX:
        raise RuntimeError(f"special-family {label} is invalid")
    return value


def table_identity(columns, key_columns, rows):
    if (not isinstance(columns, list) or not columns or len(columns) != len(set(columns))
            or any(not isinstance(column, str) or not column.isidentifier() for column in columns)
            or not isinstance(key_columns, list) or not key_columns
            or len(key_columns) != len(set(key_columns)) or not set(key_columns) <= set(columns)):
        raise RuntimeError("special-family table columns are invalid")
    if not isinstance(rows, list) or len(rows) > MAX_ROWS:
        raise RuntimeError("special-family row bound exceeded")
    indexes = [columns.index(column) for column in key_columns]
    normalized = []
    for row in rows:
        if not isinstance(row, (list, tuple)) or len(row) != len(columns):
            raise RuntimeError("special-family row shape is invalid")
        normalized.append([_value(value) for value in row])
    normalized.sort(key=lambda row: _canonical([row[index] for index in indexes]))
    keys = [[row[index] for index in indexes] for row in normalized]
    if len({_canonical(key) for key in keys}) != len(keys):
        raise RuntimeError("special-family source keys are not unique")
    if len(_canonical(normalized)) > MAX_BYTES:
        raise RuntimeError("special-family content byte bound exceeded")
    return {"row_count": len(normalized), "key_set_sha256": _sha(keys),
            "content_sha256": _sha(normalized), "rows": normalized}


def build_baseline(baseline_evidence_id, source_snapshot_id, tables):
    _digest(baseline_evidence_id, "baseline evidence id")
    if not isinstance(source_snapshot_id, str) or not source_snapshot_id:
        raise RuntimeError("special-family source snapshot is invalid")
    if not isinstance(tables, dict) or set(tables) != set(TABLES):
        raise RuntimeError("all seven special-family tables are required")
    result = {}
    total_rows = total_bytes = 0
    for name in TABLES:
        item = tables[name]
        if not isinstance(item, dict) or set(item) != {"columns", "key_columns", "rows", "schema_sha256"}:
            raise RuntimeError("special-family source table schema is invalid")
        identity = table_identity(item["columns"], item["key_columns"], item["rows"])
        total_rows += identity["row_count"]
        total_bytes += len(_canonical(identity["rows"]))
        result[name] = {"columns": item["columns"], "key_columns": item["key_columns"],
                        "schema_sha256": _digest(item["schema_sha256"], "schema digest"), **identity}
    if total_rows > MAX_ROWS or total_bytes > MAX_BYTES:
        raise RuntimeError("special-family aggregate migration bound exceeded")
    global_identity = _sha({name: {key: result[name][key] for key in
        ("row_count", "schema_sha256", "key_set_sha256", "content_sha256")} for name in TABLES})
    return {"schema_version": SCHEMA_VERSION, "baseline_evidence_id": baseline_evidence_id,
            "source_snapshot_id": source_snapshot_id, "tables": result,
            "global_content_sha256": global_identity}


def publish(baseline, *, expected_head, publisher):
    """Publish immutable table objects first and one manifest with a bounded head CAS."""
    verified = verify_baseline(baseline)
    if not isinstance(expected_head, str) or not expected_head:
        raise RuntimeError("special-family publication requires an expected head")
    receipts = []
    for name in TABLES:
        item = baseline["tables"][name]
        body = _canonical({"schema_version": SCHEMA_VERSION,
                           "baseline_evidence_id": baseline["baseline_evidence_id"],
                           "table": name, "columns": item["columns"],
                           "key_columns": item["key_columns"], "rows": item["rows"]})
        path = f"postgresql-special-families/{baseline['baseline_evidence_id']}/{name}.json"
        receipt = publisher.publish_immutable(path, body, hashlib.sha256(body).hexdigest())
        if (not isinstance(receipt, dict) or receipt.get("path") != path
                or receipt.get("sha256") != hashlib.sha256(body).hexdigest()
                or receipt.get("status") not in {"created", "verified-replay"}
                or receipt.get("row_count") != item["row_count"]
                or receipt.get("key_set_sha256") != item["key_set_sha256"]
                or receipt.get("content_sha256") != item["content_sha256"]):
            raise RuntimeError("special-family immutable publication failed")
        if type(receipt.get("bytes")) is not int or not 1 <= receipt["bytes"] <= MAX_BYTES:
            raise RuntimeError("special-family immutable publication exceeded its byte bound")
        receipts.append(receipt)
    manifest = {"schema_version": SCHEMA_VERSION,
                "baseline_evidence_id": baseline["baseline_evidence_id"],
                "source_snapshot_id": baseline["source_snapshot_id"],
                "global_content_sha256": verified["global_content_sha256"],
                "tables": [{"table": name, "path": receipts[index]["path"],
                            "sha256": receipts[index]["sha256"],
                            "row_count": baseline["tables"][name]["row_count"],
                            "key_set_sha256": baseline["tables"][name]["key_set_sha256"],
                            "content_sha256": baseline["tables"][name]["content_sha256"]}
                           for index, name in enumerate(TABLES)]}
    body = _canonical(manifest)
    committed = publisher.publish_manifest(body, expected_head=expected_head,
                                            max_attempts=MAX_CAS_ATTEMPTS)
    if (not isinstance(committed, dict) or committed.get("status") not in
            {"committed", "verified-replay"} or committed.get("sha256") !=
            hashlib.sha256(body).hexdigest() or type(committed.get("cas_attempts")) is not int
            or not 0 <= committed["cas_attempts"] <= MAX_CAS_ATTEMPTS
            or committed.get("published_last") is not True):
        raise RuntimeError("special-family manifest CAS publication failed")
    return {"schema_version": SCHEMA_VERSION,
            "baseline_evidence_id": baseline["baseline_evidence_id"],
            "source_snapshot_id": baseline["source_snapshot_id"],
            "global_content_sha256": verified["global_content_sha256"],
            "tables": [{"table": name,
                        "source": verified["table_identities"][name],
                        "target": {key: receipts[index][key] for key in
                                   ("row_count", "key_set_sha256", "content_sha256")}}
                       for index, name in enumerate(TABLES)],
            "objects": receipts, "manifest": committed}


def verify_baseline(value):
    if (not isinstance(value, dict) or set(value) != {"schema_version", "baseline_evidence_id",
            "source_snapshot_id", "tables", "global_content_sha256"}
            or value["schema_version"] != SCHEMA_VERSION):
        raise RuntimeError("special-family baseline schema is invalid")
    rebuilt = build_baseline(value["baseline_evidence_id"], value["source_snapshot_id"], {
        name: {key: value["tables"][name][key] for key in
               ("columns", "key_columns", "rows", "schema_sha256")} for name in TABLES})
    if rebuilt != value:
        raise RuntimeError("special-family baseline identity mismatch")
    return {"baseline_evidence_id": value["baseline_evidence_id"],
            "source_snapshot_id": value["source_snapshot_id"],
            "global_content_sha256": value["global_content_sha256"],
            "table_identities": {name: {key: value["tables"][name][key] for key in
                ("row_count", "schema_sha256", "key_set_sha256", "content_sha256")}
                for name in TABLES}}


def verify_evidence(evidence, baseline):
    identity = verify_baseline(baseline)
    if (not isinstance(evidence, dict) or set(evidence) != {"schema_version",
            "baseline_evidence_id", "source_snapshot_id", "global_content_sha256",
            "tables", "objects", "manifest"} or evidence["schema_version"] != SCHEMA_VERSION
            or evidence["baseline_evidence_id"] != identity["baseline_evidence_id"]
            or evidence["source_snapshot_id"] != identity["source_snapshot_id"]
            or evidence["global_content_sha256"] != identity["global_content_sha256"]):
        raise RuntimeError("special-family migration is not bound to the baseline")
    tables = evidence["tables"]
    if (not isinstance(tables, list) or len(tables) != len(TABLES)
            or [item.get("table") for item in tables] != list(TABLES)):
        raise RuntimeError("special-family target table coverage is incomplete")
    for item in tables:
        expected = identity["table_identities"][item["table"]]
        target_expected = {key: expected[key] for key in
                           ("row_count", "key_set_sha256", "content_sha256")}
        if set(item) != {"table", "source", "target"} or item["source"] != expected \
                or item["target"] != target_expected:
            raise RuntimeError("special-family source and target identities differ")
    objects = evidence["objects"]
    if (not isinstance(objects, list) or len(objects) != len(TABLES)
            or [item.get("path", "").rsplit("/", 1)[-1] for item in objects]
               != [name + ".json" for name in TABLES]
            or any(item.get("status") not in {"created", "verified-replay"}
                   or _digest(item.get("sha256"), "object digest") != item["sha256"]
                   for item in objects)):
        raise RuntimeError("special-family immutable object evidence is incomplete")
    for index, name in enumerate(TABLES):
        item = baseline["tables"][name]
        body = _canonical({"schema_version": SCHEMA_VERSION,
                           "baseline_evidence_id": baseline["baseline_evidence_id"],
                           "table": name, "columns": item["columns"],
                           "key_columns": item["key_columns"], "rows": item["rows"]})
        if objects[index]["sha256"] != hashlib.sha256(body).hexdigest():
            raise RuntimeError("special-family immutable object identity mismatch")
    manifest = evidence["manifest"]
    if (not isinstance(manifest, dict) or manifest.get("status") not in
            {"committed", "verified-replay"} or _digest(manifest.get("sha256"),
            "manifest digest") != manifest["sha256"] or type(manifest.get("cas_attempts")) is not int
            or not 0 <= manifest["cas_attempts"] <= MAX_CAS_ATTEMPTS
            or manifest.get("published_last") is not True):
        raise RuntimeError("special-family manifest evidence is invalid")
    manifest_body = _canonical({"schema_version": SCHEMA_VERSION,
        "baseline_evidence_id": baseline["baseline_evidence_id"],
        "source_snapshot_id": baseline["source_snapshot_id"],
        "global_content_sha256": identity["global_content_sha256"],
        "tables": [{"table": name, "path": objects[index]["path"],
                    "sha256": objects[index]["sha256"],
                    "row_count": baseline["tables"][name]["row_count"],
                    "key_set_sha256": baseline["tables"][name]["key_set_sha256"],
                    "content_sha256": baseline["tables"][name]["content_sha256"]}
                   for index, name in enumerate(TABLES)]})
    if manifest["sha256"] != hashlib.sha256(manifest_body).hexdigest():
        raise RuntimeError("special-family manifest identity mismatch")
    return identity
