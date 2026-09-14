import hashlib
import json
import os
from unittest.mock import patch

import pytest

from services import postgresql_special_family_restore as restore


def deletion():
    schemas = {table: hashlib.sha256(table.encode()).hexdigest() for table in restore.TABLES}
    return {"context": {"source": {"snapshot_id": "pg-snapshot-1", "cache_rows": 0,
        "snapshot_rows": 0, "cache_schema_sha256": schemas["context_answer_cache"],
        "snapshot_schema_sha256": schemas["context_snapshots"]}},
        "dlm": {"source": {"snapshot_id": "pg-snapshot-1",
        "rows": {table: (1 if table == "dlm_artifact" else 0)
                 for table in restore.retirement.DLM_TABLES},
        "schema_sha256": {table: schemas[table]
                          for table in restore.retirement.DLM_TABLES}}}}


def bundle(value=None):
    evidence = deletion(); rows = value if value is not None else [["dataset-1"]]
    tables = []
    for table in restore.TABLES:
        current = rows if table == "dlm_artifact" else []
        schema = (evidence["context"]["source"].get("cache_schema_sha256")
                  if table == "context_answer_cache" else
                  evidence["context"]["source"].get("snapshot_schema_sha256")
                  if table == "context_snapshots" else
                  evidence["dlm"]["source"]["schema_sha256"][table])
        tables.append({"table": table, "columns": ["dataset_id"], "rows": current,
            "row_count": len(current), "rows_sha256": hashlib.sha256(
                json.dumps(current, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
            "schema_sha256": schema})
    result = {"schema_version": 1, "source_id": "pg-snapshot-1", "tables": tables}
    result["bundle_sha256"] = hashlib.sha256(json.dumps(
        result, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return result


def test_restores_only_with_reviewed_identity_and_within_bounds():
    value = bundle(); seen = []
    with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RESTORE_ENABLED": "true",
             "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True):
        result = restore.restore(value, deletion(), expected_bundle_sha256=value["bundle_sha256"],
            cutover_revision="helm-7", runner=lambda tables, schemas: seen.append(schemas) or {
                table: item["row_count"] for table, item in tables.items()}, clock=lambda: 1)
    assert result == {"cutover_revision": "helm-7", "rollback_operation_count": 1}
    assert seen


def test_rejects_unreviewed_bundle_before_target_access():
    value = bundle(); called = []
    with patch.dict(os.environ, {"KAVEON_SPECIAL_FAMILY_RESTORE_ENABLED": "true",
             "KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED": "true"}, clear=True), pytest.raises(
                 RuntimeError, match="reviewed evidence"):
        restore.restore(value, deletion(), expected_bundle_sha256="f" * 64,
            cutover_revision="helm-7", runner=lambda *_: called.append(True))
    assert called == []


def test_requires_fence_and_explicit_enablement():
    value = bundle(); called = []
    with patch.dict(os.environ, {}, clear=True), pytest.raises(RuntimeError, match="enablement"):
        restore.restore(value, deletion(), expected_bundle_sha256=value["bundle_sha256"],
            cutover_revision="helm-7", runner=lambda *_: called.append(True))
    assert called == []


def test_rejects_payload_that_no_longer_matches_its_digest():
    value = bundle(); value["tables"][-1]["rows"] = [["tampered"]]
    with pytest.raises(RuntimeError, match="integrity"):
        restore.validate_bundle(value, deletion(), value["bundle_sha256"])
