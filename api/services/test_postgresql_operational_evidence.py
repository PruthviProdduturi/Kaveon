import copy
import hashlib
import json
from datetime import datetime, timezone

import pytest

from services import postgresql_operational_evidence as evidence
from services import postgresql_retirement_gate as retirement
from services import postgresql_baseline_identity as canonical
from services import postgresql_special_family_migration as migration


DIGEST = "a" * 64
NOW = datetime(2026, 9, 14, 19, 0, tzinfo=timezone.utc)


def _observations():
    probes = [{"family": family, "passed": True} for family in sorted(retirement.AUTHORITY_FAMILIES)]
    values = {
        "source_watermark": ({"watermark": 42}, {"source_snapshot": DIGEST, "watermark_observed": 42}),
        "outbox_drain": ({"pending_events": 0}, {"query_id": 7, "watermark": 42, "pending_before": 3, "pending_after": 0}),
        "write_fence": ({"enabled": True}, {"deployment_revision": "api@abc123", "readonly_probe_passed": True, "family_probes": probes}),
        "shadow_parity": ({"matched": True}, {"source_snapshot": DIGEST, "target_snapshot": "b" * 64, "family_probes": probes, "mismatch_count": 0}),
        "restart_recovery": ({"verified": True}, {"postgresql_unavailable": True, "api_restarted": True, "studio_restarted": True, "probe_count": 21, "service_state_sha256": "e" * 64, "state_sha256_before": DIGEST, "state_sha256_after": DIGEST, "state_record_count_before":24,"state_record_count_after":24}),
        "rollback": ({"verified": True}, {"cutover_revision": "api@abc123", "target_writes_fenced": True, "source_reads_restored": True, "source_writes_restored": True, "state_sha256_before": DIGEST, "state_sha256_after": DIGEST, "duration_seconds": 90,"rollback_operation_count":16,"rollback_operation_limit":100}),
        "backup_identity": ({"backup_id": "snapshot-1", "backup_sha256": "c" * 64, "restore_verified": True}, {"backup_id": "snapshot-1", "backup_sha256": "c" * 64, "restore_job_id": "restore-1", "source_inventory_sha256": DIGEST, "restored_inventory_sha256": DIGEST, "restored_table_count": 24,"immutable_prefix":"https://account.dfs.core.windows.net/container/backups/snapshot-1/","manifest_sha256":"d"*64,"restore_executed":True}),
        "durable_checkpoint": ({"verified": True}, {"checkpoint_sha256_before": DIGEST, "checkpoint_sha256_after": DIGEST, "pod_uid_before": "pod-1", "pod_uid_after": "pod-2", "next_index_before": 8, "next_index_after": 10, "resume_completed": True}),
    }
    binding = {"verified": True, "baseline_evidence_id": "fresh-baseline-1",
               "baseline_sha256": DIGEST}
    values.update({
        "postgresql_baseline_identity": (binding, {"baseline_evidence_id": "fresh-baseline-1", "baseline_sha256": DIGEST, "table_count": 7, "dataset17_utf8_verified": True}),
        "baseline_restore_qualification": (binding, {"baseline_evidence_id": "fresh-baseline-1", "baseline_sha256": DIGEST, "restored_sha256": DIGEST, "table_count": 7, "restore_job_id": "restore-fresh-1", "exact_match": True}),
        "lossless_full_migration": (binding, {"baseline_evidence_id": "fresh-baseline-1", "baseline_sha256": DIGEST, "source_sha256": DIGEST, "target_sha256": DIGEST, "table_count": 7, "pending_events": 0, "failed_events": 0, "manifest_published_last": True}),
        "pre_delete_baseline_recheck": (binding, {"baseline_evidence_id": "fresh-baseline-1", "baseline_sha256": DIGEST, "observed_sha256": DIGEST, "table_count": 7, "writes_fenced": True, "outbox_pending": 0}),
        "exact_post_rollback_identity": (binding, {"baseline_evidence_id": "fresh-baseline-1", "baseline_sha256": DIGEST, "restored_sha256": DIGEST, "table_count": 7, "exact_match": True}),
    })
    return values


def _receipt(gate, details, observation):
    value = {"schema_version": evidence.SCHEMA_VERSION, "gate": gate, "checked_at": "2026-09-14T18:00:00Z",
             "evidence_id": f"{gate}-run-1", "details": details, "observation": observation}
    value["receipt_sha256"] = hashlib.sha256(evidence._canonical(value)).hexdigest()
    return value


def _write_receipts(directory):
    values = {}
    for gate, (details, observation) in _observations().items():
        values[gate] = _receipt(gate, details, copy.deepcopy(observation))
        (directory / f"{gate}.json").write_text(json.dumps(values[gate]), encoding="utf-8")
    return values


def test_collect_requires_and_validates_all_live_receipts(tmp_path):
    _write_receipts(tmp_path)
    gates, operational = evidence.collect(tmp_path, now=NOW, max_rollback_seconds=120)
    assert set(gates) == set(retirement.GLOBAL_GATE_NAMES)
    assert all(item["status"] == "passed" for item in gates.values())
    assert operational["postgresql_unavailable_restart"]["status"] == "passed"
    assert operational["postgresql_baseline_identity"]["baseline_evidence_id"] == "fresh-baseline-1"
    assert len(operational["receipt_set_sha256"]) == 64


def test_lossless_producer_flows_through_receipt_collector_and_summary(tmp_path):
    columns = [{"name": "id", "type": "integer", "nullable": False, "ordinal": 1}]
    tables = [canonical.table_identity(name, columns, ["id"], [{"id": 1}])
              for name in migration.TABLES if name != "dlm_artifact"]
    artifact_columns = [
        {"name": "dataset_id", "type": "text", "nullable": False, "ordinal": 1},
        {"name": "manifest", "type": "jsonb", "nullable": False, "ordinal": 2}]
    tables.append(canonical.table_identity("dlm_artifact", artifact_columns, ["dataset_id"],
        [{"dataset_id": "17", "manifest": {"name": "Climate × Energy"}}]))
    baseline = canonical.build("qualified-source", tables)
    class Publisher:
        def publish_immutable(self, path, body, sha256):
            table = json.loads(body)["table"]
            return {"path": path, "sha256": sha256, "bytes": len(body), "status": "created",
                    "row_count": table["row_count"], "key_set_sha256": table["key_sha256"],
                    "content_sha256": table["content_sha256"]}
        def publish_manifest(self, body, **_kwargs):
            return {"sha256": hashlib.sha256(body).hexdigest(), "status": "committed",
                    "cas_attempts": 1, "published_last": True}
    produced = migration.publish(baseline, expected_head="absent", publisher=Publisher(),
                                 source_pending_events=0)
    values = _write_receipts(tmp_path)
    baseline_id = produced["baseline_evidence_id"]
    for gate, observation in {
        "postgresql_baseline_identity": {"baseline_evidence_id": baseline_id,
            "baseline_sha256": baseline_id, "table_count": 7, "dataset17_utf8_verified": True},
        "baseline_restore_qualification": {"baseline_evidence_id": baseline_id,
            "baseline_sha256": baseline_id, "restored_sha256": baseline_id, "table_count": 7,
            "restore_job_id": "restore-1", "exact_match": True},
        "lossless_full_migration": produced["operational_observation"],
        "pre_delete_baseline_recheck": {"baseline_evidence_id": baseline_id,
            "baseline_sha256": baseline_id, "observed_sha256": baseline_id, "table_count": 7,
            "writes_fenced": True, "outbox_pending": 0},
        "exact_post_rollback_identity": {"baseline_evidence_id": baseline_id,
            "baseline_sha256": baseline_id, "restored_sha256": baseline_id,
            "table_count": 7, "exact_match": True},
    }.items():
        receipt = evidence.receipt_from_observation(gate, observation,
            checked_at="2026-09-14T18:00:00Z", evidence_id=f"live:{gate}")
        (tmp_path / f"{gate}.json").write_text(json.dumps(receipt), encoding="utf-8")
    _, summary = evidence.collect(tmp_path, now=NOW, max_rollback_seconds=120)
    assert summary["lossless_full_migration"]["baseline_sha256"] == baseline_id
    assert summary["pre_delete_baseline_recheck"]["baseline_evidence_id"] == baseline_id


def test_missing_or_tampered_receipt_fails_closed(tmp_path):
    values = _write_receipts(tmp_path)
    (tmp_path / "durable_checkpoint.json").unlink()
    with pytest.raises(RuntimeError, match="missing or oversized"):
        evidence.collect(tmp_path, now=NOW)
    tampered = values["restart_recovery"]
    tampered["observation"]["probe_count"] = 99
    (tmp_path / "restart_recovery.json").write_text(json.dumps(tampered), encoding="utf-8")
    with pytest.raises(RuntimeError, match="digest mismatch"):
        evidence.load_receipt(tmp_path / "restart_recovery.json", "restart_recovery",
                              now=NOW, max_age_hours=24, max_rollback_seconds=900)


def test_write_fence_requires_each_authority_family_exactly_once(tmp_path):
    values = _write_receipts(tmp_path)
    receipt = values["write_fence"]
    receipt["observation"]["family_probes"].pop()
    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    receipt["receipt_sha256"] = hashlib.sha256(evidence._canonical(unsigned)).hexdigest()
    (tmp_path / "write_fence.json").write_text(json.dumps(receipt), encoding="utf-8")
    with pytest.raises(RuntimeError, match="all authority families exactly once"):
        evidence.collect(tmp_path, now=NOW)


@pytest.mark.parametrize("gate,mutation,message", [
    ("restart_recovery", lambda value: value.update(postgresql_unavailable=False), "PostgreSQL-independent"),
    ("rollback", lambda value: value.update(duration_seconds=901), "recovery bound"),
    ("backup_identity", lambda value: value.update(restored_inventory_sha256="b" * 64), "did not reconcile"),
    ("durable_checkpoint", lambda value: value.update(pod_uid_after="pod-1"), "pod replacement"),
])
def test_operational_claims_require_matching_observations(tmp_path, gate, mutation, message):
    values = _write_receipts(tmp_path)
    receipt = values[gate]
    mutation(receipt["observation"])
    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    receipt["receipt_sha256"] = hashlib.sha256(evidence._canonical(unsigned)).hexdigest()
    path = tmp_path / f"{gate}.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    with pytest.raises(RuntimeError, match=message):
        evidence.load_receipt(path, gate, now=NOW, max_age_hours=24, max_rollback_seconds=900)


def test_stale_operational_receipt_fails_closed(tmp_path):
    values = _write_receipts(tmp_path)
    receipt = values["durable_checkpoint"]
    receipt["checked_at"] = "2026-09-12T18:00:00Z"
    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    receipt["receipt_sha256"] = hashlib.sha256(evidence._canonical(unsigned)).hexdigest()
    path = tmp_path / "durable_checkpoint.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    with pytest.raises(RuntimeError, match="not fresh"):
        evidence.load_receipt(path, "durable_checkpoint", now=NOW,
                              max_age_hours=24, max_rollback_seconds=900)


def test_fresh_baseline_receipts_must_share_one_identity(tmp_path):
    values = _write_receipts(tmp_path)
    receipt = values["exact_post_rollback_identity"]
    receipt["details"]["baseline_sha256"] = "b" * 64
    receipt["observation"]["baseline_sha256"] = "b" * 64
    receipt["observation"]["restored_sha256"] = "b" * 64
    unsigned = {key: value for key, value in receipt.items() if key != "receipt_sha256"}
    receipt["receipt_sha256"] = hashlib.sha256(evidence._canonical(unsigned)).hexdigest()
    (tmp_path / "exact_post_rollback_identity.json").write_text(json.dumps(receipt))
    with pytest.raises(RuntimeError, match="bound to one identity"):
        evidence.collect(tmp_path, now=NOW)


@pytest.mark.parametrize("gate,field,value,message", [
    ("postgresql_baseline_identity", "dataset17_utf8_verified", False, "UTF-8"),
    ("baseline_restore_qualification", "restored_sha256", "b" * 64, "not exact"),
    ("lossless_full_migration", "manifest_published_last", False, "incomplete"),
    ("pre_delete_baseline_recheck", "writes_fenced", False, "unfenced"),
    ("exact_post_rollback_identity", "exact_match", False, "not exact"),
])
def test_fresh_baseline_gate_claims_fail_closed(tmp_path, gate, field, value, message):
    receipts = _write_receipts(tmp_path)
    receipt = receipts[gate]
    receipt["observation"][field] = value
    unsigned = {key: item for key, item in receipt.items() if key != "receipt_sha256"}
    receipt["receipt_sha256"] = hashlib.sha256(evidence._canonical(unsigned)).hexdigest()
    path = tmp_path / f"{gate}.json"; path.write_text(json.dumps(receipt))
    with pytest.raises(RuntimeError, match=message):
        evidence.load_receipt(path, gate, now=NOW, max_age_hours=24,
                              max_rollback_seconds=900)
