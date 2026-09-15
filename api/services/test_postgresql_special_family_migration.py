import copy
import unittest
from datetime import datetime, timezone
from decimal import Decimal

from services import postgresql_special_family_migration as migration


def source():
    return {name: {"columns": ["id", "payload"], "key_columns": ["id"],
                   "rows": [[2, {"text": "cafÃ©"}], [1, {"text": "café"}]],
                   "schema_sha256": "a" * 64} for name in migration.TABLES}


class Publisher:
    def __init__(self, replay=False, bad=False):
        self.events = []; self.replay = replay; self.bad = bad

    def publish_immutable(self, path, body, sha256):
        self.events.append(("object", path))
        value = __import__("json").loads(body)
        identity = migration.table_identity(value["columns"], value["key_columns"], value["rows"])
        return {"path": path, "sha256": "0" * 64 if self.bad else sha256,
                "status": "verified-replay" if self.replay else "created", "bytes": len(body),
                **{key: identity[key] for key in
                   ("row_count", "key_set_sha256", "content_sha256")}}

    def publish_manifest(self, body, *, expected_head, max_attempts):
        self.events.append(("manifest", expected_head))
        return {"sha256": __import__("hashlib").sha256(body).hexdigest(),
                "status": "verified-replay" if self.replay else "committed", "cas_attempts": 1,
                "published_last": True}


class Tests(unittest.TestCase):
    def test_all_seven_are_canonical_and_manifest_is_last(self):
        baseline = migration.build_baseline("b" * 64, "pg:1", source())
        self.assertEqual(set(baseline["tables"]), set(migration.TABLES))
        self.assertEqual(baseline["tables"]["dlm_answers"]["rows"][0][0], 1)
        publisher = Publisher()
        evidence = migration.publish(baseline, expected_head="head:4", publisher=publisher)
        self.assertEqual([event[0] for event in publisher.events],
                         ["object"] * 7 + ["manifest"])
        self.assertEqual(migration.verify_evidence(evidence, baseline)["baseline_evidence_id"],
                         "b" * 64)

    def test_unicode_bytes_and_same_count_content_changes_are_detected(self):
        first = migration.build_baseline("b" * 64, "pg:1", source())
        changed_source = source(); changed_source["dlm_answers"]["rows"][0][1]["text"] = "cafe"
        second = migration.build_baseline("b" * 64, "pg:1", changed_source)
        self.assertNotEqual(first["tables"]["dlm_answers"]["content_sha256"],
                            second["tables"]["dlm_answers"]["content_sha256"])
        different_utf8 = source()
        different_utf8["dlm_answers"]["rows"][1][1]["text"] = "cafe\u0301"
        third = migration.build_baseline("b" * 64, "pg:1", different_utf8)
        self.assertNotEqual(first["tables"]["dlm_answers"]["content_sha256"],
                            third["tables"]["dlm_answers"]["content_sha256"])

    def test_typed_values_are_stable_and_duplicate_keys_fail(self):
        rows = [[1, Decimal("1.00")], [2, b"x"], [3, datetime(2026, 9, 14, tzinfo=timezone.utc)]]
        result = migration.table_identity(["id", "value"], ["id"], rows)
        self.assertEqual(result["row_count"], 3)
        with self.assertRaisesRegex(RuntimeError, "not unique"):
            migration.table_identity(["id", "value"], ["id"], [[1, "a"], [1, "b"]])

    def test_replay_is_exact_and_divergent_object_fails_before_manifest(self):
        baseline = migration.build_baseline("b" * 64, "pg:1", source())
        replay = Publisher(replay=True)
        migration.verify_evidence(migration.publish(baseline, expected_head="head:4",
                                                     publisher=replay), baseline)
        bad = Publisher(bad=True)
        with self.assertRaisesRegex(RuntimeError, "immutable publication failed"):
            migration.publish(baseline, expected_head="head:4", publisher=bad)
        self.assertNotIn("manifest", [event[0] for event in bad.events])

    def test_wrong_baseline_and_cas_bounds_are_rejected(self):
        baseline = migration.build_baseline("b" * 64, "pg:1", source())
        evidence = migration.publish(baseline, expected_head="head:4", publisher=Publisher())
        tampered = copy.deepcopy(evidence); tampered["baseline_evidence_id"] = "c" * 64
        with self.assertRaisesRegex(RuntimeError, "not bound"):
            migration.verify_evidence(tampered, baseline)
        evidence["manifest"]["cas_attempts"] = migration.MAX_CAS_ATTEMPTS + 1
        with self.assertRaisesRegex(RuntimeError, "manifest evidence"):
            migration.verify_evidence(evidence, baseline)


if __name__ == "__main__": unittest.main()
