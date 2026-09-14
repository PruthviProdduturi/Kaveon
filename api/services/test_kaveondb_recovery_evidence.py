import pytest

from services import kaveondb_recovery_evidence as recovery


DIGEST = "a" * 64


def test_state_identity_is_content_free_order_independent():
    records = [{"kind":"dataset","id":"2","revision":3,"document_sha256":DIGEST},
               {"kind":"chart","id":"1","revision":1,"document_sha256":"b"*64}]
    first = recovery.state_identity(records)
    assert first == recovery.state_identity(list(reversed(records)))
    assert first["record_count"] == 2 and set(first) == {"record_count","state_sha256"}
    with pytest.raises(RuntimeError,match="inventory record"):
        recovery.state_identity([{**records[0],"document":{"name":"must not appear"}}])


def test_immutable_adls_manifest_is_integrity_bound():
    manifest={"schema_version":1,"backup_id":"backup-1",
        "immutable_prefix":"https://account.dfs.core.windows.net/container/backups/backup-1/",
        "state_sha256":DIGEST,"record_count":2,
        "objects":[{"path":"manifests/head.json","etag":"etag-1","size":12,"sha256":"b"*64}]}
    result=recovery.validate_backup_manifest(manifest)
    assert result["object_count"]==1 and len(result["manifest_sha256"])==64
    manifest["immutable_prefix"] += "?sig=credential"
    with pytest.raises(RuntimeError,match="immutable ADLS"):
        recovery.validate_backup_manifest(manifest)


def test_rollback_control_is_bounded():
    control={"cutover_revision":"api@abc","expected_state_sha256":DIGEST,
             "max_operations":100,"max_duration_seconds":900}
    assert recovery.validate_rollback_control(control)==control
    control["max_operations"]=10001
    with pytest.raises(RuntimeError,match="unbounded"):
        recovery.validate_rollback_control(control)
