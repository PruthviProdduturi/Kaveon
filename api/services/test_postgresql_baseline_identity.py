import copy
from datetime import datetime, timezone
from decimal import Decimal

import pytest

from services import postgresql_baseline_identity as identity
from services import postgresql_operational_evidence as operational


def columns():
    return [
        {"name": "id", "type": "bigint", "nullable": False, "ordinal": 1},
        {"name": "manifest", "type": "jsonb", "nullable": False, "ordinal": 2},
        {"name": "amount", "type": "numeric", "nullable": True, "ordinal": 3},
        {"name": "captured", "type": "timestamp with time zone", "nullable": False, "ordinal": 4},
        {"name": "raw", "type": "bytea", "nullable": True, "ordinal": 5},
    ]


def test_canonical_types_and_primary_key_order_are_stable():
    rows = [
        {"id": 2, "manifest": '{"b":2,"a":"é"}', "amount": Decimal("1.2300"),
         "captured": datetime(2026, 9, 14, 12, tzinfo=timezone.utc), "raw": b"\x00\xff"},
        {"id": 1, "manifest": {"a": None}, "amount": None,
         "captured": "2026-09-14T05:00:00-07:00", "raw": None},
    ]
    first = identity.table_identity("sample", columns(), ["id"], rows)
    second = identity.table_identity("sample", columns(), ["id"], list(reversed(rows)))
    assert first == second
    assert first["rows"][0][0] == ["integer", "1"]
    assert first["rows"][1][2] == ["numeric", "1.23"]
    assert identity.encode_value(Decimal("100"), "numeric") == ["numeric", "100"]
    assert first["rows"][1][3] == ["timestamp", "2026-09-14T12:00:00.000000Z"]
    assert first["rows"][1][4] == ["bytea", "AP8="]


def dlm_payload(name="Climate × Energy"):
    table = identity.table_identity("dlm_artifact", [
        {"name": "dataset_id", "type": "text", "nullable": False, "ordinal": 1},
        {"name": "manifest", "type": "jsonb", "nullable": False, "ordinal": 2},
    ], ["dataset_id"], [{"dataset_id": "17", "manifest": {"name": name}}])
    return identity.build("pg-snapshot-1", [table])


def test_global_identity_and_exact_restore_qualification():
    source = dlm_payload(); restored = copy.deepcopy(source)
    result = identity.qualify_restore(source, restored)
    assert result["restore_verified"] is True
    assert result["source_inventory_sha256"] == result["restored_inventory_sha256"]
    assert result["restored_table_count"] == 1


def test_exact_restore_feeds_strict_backup_evidence():
    payload = dlm_payload()
    observation = identity.backup_identity_observation(payload, copy.deepcopy(payload),
        backup_id="baseline-1", restore_job_id="restore-1",
        immutable_prefix="https://account.blob.core.windows.net/private/backups/baseline-1/",
        manifest_sha256="a" * 64)
    receipt = operational.receipt_from_observation("backup_identity", observation,
        checked_at="2026-09-14T23:00:00Z", evidence_id="baseline-restore-1")
    assert receipt["details"]["restore_verified"] is True
    assert receipt["observation"]["source_inventory_sha256"] == payload["manifest"]["global_sha256"]


def test_dataset17_mojibake_fails_closed():
    with pytest.raises(RuntimeError, match="UTF-8 sentinel"):
        identity.require_dataset17_sentinel(dlm_payload("Climate Ã— Energy"))


def test_tamper_schema_content_key_and_global_hash_fail_closed():
    for mutation in ("schema", "content", "key", "global"):
        payload = dlm_payload(); changed = copy.deepcopy(payload)
        if mutation == "schema": changed["tables"][0]["columns"][0]["type"] = "integer"
        elif mutation == "content": changed["tables"][0]["rows"][0][1] = ["json", {"name": "changed"}]
        elif mutation == "key": changed["tables"][0]["key_sha256"] = "0" * 64
        else: changed["manifest"]["global_sha256"] = "0" * 64
        with pytest.raises(RuntimeError, match="identity"):
            identity.validate(changed)


def test_rejects_false_row_count_even_when_manifest_is_rehashed():
    payload = dlm_payload()
    payload["tables"][0]["row_count"] = 2
    payload["manifest"]["tables"][0]["row_count"] = 2
    payload["manifest"]["row_count"] = 2
    payload["manifest"]["global_sha256"] = identity._sha(payload["manifest"]["tables"])
    with pytest.raises(RuntimeError, match="table schema"):
        identity.validate(payload)


def test_rejects_duplicate_keys_nonfinite_numeric_and_naive_timestamptz():
    text_columns = [{"name": "id", "type": "text", "nullable": False, "ordinal": 1}]
    with pytest.raises(RuntimeError, match="duplicate"):
        identity.table_identity("t", text_columns, ["id"], [{"id": "x"}, {"id": "x"}])
    with pytest.raises(RuntimeError, match="finite"):
        identity.encode_value(float("nan"), "double precision")
    with pytest.raises(RuntimeError, match="timezone"):
        identity.encode_value(datetime(2026, 1, 1), "timestamp with time zone")
