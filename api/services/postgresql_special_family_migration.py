"""Lossless, manifest-last publication of canonical special-family baselines."""

from __future__ import annotations

import hashlib
import json

from services import postgresql_baseline_identity as baseline_identity

TABLES = ("context_answer_cache", "context_snapshots", "dlm_answers",
          "dlm_artifact", "dlm_router", "dlm_sketch", "dlm_value_index")
SCHEMA_VERSION = 2
MAX_CAS_ATTEMPTS = 8


def _canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False, allow_nan=False).encode("utf-8")


def _sha_bytes(value): return hashlib.sha256(value).hexdigest()


def verify_baseline(payload):
    manifest = baseline_identity.validate(payload)
    found = tuple(table["name"] for table in payload["tables"])
    if set(found) != set(TABLES) or len(found) != len(TABLES):
        raise RuntimeError("canonical baseline must contain all seven special-family tables")
    identities = {table["name"]: {
        "row_count": table["row_count"], "schema_sha256": table["schema_sha256"],
        "key_set_sha256": table["key_sha256"], "content_sha256": table["content_sha256"]}
        for table in payload["tables"]}
    return {"baseline_evidence_id": _sha_bytes(baseline_identity._json(payload)),
            "source_snapshot_id": manifest["source_id"],
            "global_content_sha256": manifest["global_sha256"],
            "table_identities": identities, "encoding": manifest["encoding"]}


def raw_table_identity(table, rows):
    """Hash fresh PostgreSQL rows with the exact baseline encoding contract."""
    names = [column["name"] for column in table["columns"]]
    dictionaries = [dict(zip(names, row)) for row in rows]
    value = baseline_identity.table_identity(table["name"], table["columns"],
                                             table["primary_key"], dictionaries)
    return {"row_count": value["row_count"], "schema_sha256": value["schema_sha256"],
            "key_set_sha256": value["key_sha256"], "content_sha256": value["content_sha256"]}


def _table_object(identity, table):
    return {"schema_version": SCHEMA_VERSION, "encoding": identity["encoding"],
            "baseline_evidence_id": identity["baseline_evidence_id"], "table": table}


def publish(payload, *, expected_head, publisher):
    identity = verify_baseline(payload)
    if not isinstance(expected_head, str) or not expected_head:
        raise RuntimeError("special-family publication requires an expected head ETag or 'absent'")
    receipts = []
    table_by_name = {table["name"]: table for table in payload["tables"]}
    for name in TABLES:
        table = table_by_name[name]
        body = _canonical(_table_object(identity, table))
        path = f"objects/{identity['baseline_evidence_id']}/{name}.json"
        receipt = publisher.publish_immutable(path, body, _sha_bytes(body))
        expected = identity["table_identities"][name]
        if (not isinstance(receipt, dict) or receipt.get("path") != path
                or receipt.get("sha256") != _sha_bytes(body)
                or receipt.get("status") not in {"created", "verified-replay"}
                or type(receipt.get("bytes")) is not int or not 1 <= receipt["bytes"]
                    <= baseline_identity.MAX_PAYLOAD_BYTES
                or any(receipt.get(key) != expected[key] for key in
                       ("row_count", "key_set_sha256", "content_sha256"))):
            raise RuntimeError("special-family immutable publication failed readback verification")
        receipts.append(receipt)
    manifest_value = {"schema_version": SCHEMA_VERSION, "encoding": identity["encoding"],
                "baseline_evidence_id": identity["baseline_evidence_id"],
                "source_snapshot_id": identity["source_snapshot_id"],
                "global_content_sha256": identity["global_content_sha256"],
                "tables": [{"table": name, "path": receipts[index]["path"],
                            "sha256": receipts[index]["sha256"],
                            **identity["table_identities"][name]}
                           for index, name in enumerate(TABLES)]}
    body = _canonical(manifest_value)
    committed = publisher.publish_manifest(body, expected_head=expected_head,
                                            max_attempts=MAX_CAS_ATTEMPTS)
    if (not isinstance(committed, dict) or committed.get("status") not in
            {"committed", "verified-replay"} or committed.get("sha256") != _sha_bytes(body)
            or type(committed.get("cas_attempts")) is not int
            or not 0 <= committed["cas_attempts"] <= MAX_CAS_ATTEMPTS
            or committed.get("published_last") is not True):
        raise RuntimeError("special-family manifest CAS publication failed")
    evidence = {"schema_version": SCHEMA_VERSION, **identity,
                "tables": [{"table": name,
                            "source": dict(identity["table_identities"][name]),
                            "target": dict(identity["table_identities"][name])}
                           for name in TABLES], "objects": receipts, "manifest": committed,
                "operational_observation": {
                    "baseline_evidence_id": identity["baseline_evidence_id"],
                    "baseline_sha256": identity["global_content_sha256"],
                    "source_sha256": identity["global_content_sha256"],
                    "target_sha256": identity["global_content_sha256"],
                    "table_count": len(TABLES), "pending_events": 0, "failed_events": 0,
                    "manifest_published_last": True}}
    verify_evidence(evidence, payload)
    return evidence


def verify_evidence(evidence, payload):
    identity = verify_baseline(payload)
    if (not isinstance(evidence, dict) or set(evidence) != {"schema_version",
            "baseline_evidence_id", "source_snapshot_id", "global_content_sha256",
            "table_identities", "encoding", "tables", "objects", "manifest",
            "operational_observation"}
            or evidence["schema_version"] != SCHEMA_VERSION
            or any(evidence[key] != identity[key] for key in identity)):
        raise RuntimeError("special-family migration is not bound to the canonical baseline")
    if evidence["operational_observation"] != {
            "baseline_evidence_id": identity["baseline_evidence_id"],
            "baseline_sha256": identity["global_content_sha256"],
            "source_sha256": identity["global_content_sha256"],
            "target_sha256": identity["global_content_sha256"], "table_count": len(TABLES),
            "pending_events": 0, "failed_events": 0, "manifest_published_last": True}:
        raise RuntimeError("special-family operational migration observation is invalid")
    tables, objects = evidence["tables"], evidence["objects"]
    if (not isinstance(tables, list) or [item.get("table") for item in tables] != list(TABLES)
            or not isinstance(objects, list) or len(objects) != len(TABLES)):
        raise RuntimeError("special-family target table coverage is incomplete")
    table_by_name = {table["name"]: table for table in payload["tables"]}
    for index, name in enumerate(TABLES):
        expected = identity["table_identities"][name]
        if tables[index] != {"table": name, "source": expected, "target": expected}:
            raise RuntimeError("special-family source and target identities differ")
        body = _canonical(_table_object(identity, table_by_name[name]))
        item = objects[index]
        if (item.get("path") != f"objects/{identity['baseline_evidence_id']}/{name}.json"
                or item.get("sha256") != _sha_bytes(body)
                or item.get("status") not in {"created", "verified-replay"}):
            raise RuntimeError("special-family immutable object identity mismatch")
    manifest_value = {"schema_version": SCHEMA_VERSION, "encoding": identity["encoding"],
        "baseline_evidence_id": identity["baseline_evidence_id"],
        "source_snapshot_id": identity["source_snapshot_id"],
        "global_content_sha256": identity["global_content_sha256"],
        "tables": [{"table": name, "path": objects[index]["path"],
                    "sha256": objects[index]["sha256"], **identity["table_identities"][name]}
                   for index, name in enumerate(TABLES)]}
    manifest = evidence["manifest"]
    if (not isinstance(manifest, dict) or manifest.get("sha256") !=
            _sha_bytes(_canonical(manifest_value)) or manifest.get("status") not in
            {"committed", "verified-replay"} or type(manifest.get("cas_attempts")) is not int
            or not 0 <= manifest["cas_attempts"] <= MAX_CAS_ATTEMPTS
            or manifest.get("published_last") is not True):
        raise RuntimeError("special-family manifest evidence is invalid")
    return identity
