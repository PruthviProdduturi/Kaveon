import copy
import hashlib
import json
import unittest

from services import postgresql_baseline_identity as canonical
from services import postgresql_special_family_migration as migration


def payload(text="café"):
    columns = [{"name": "id", "type": "integer", "nullable": False, "ordinal": 1},
               {"name": "payload", "type": "text", "nullable": False, "ordinal": 2}]
    tables = [canonical.table_identity(name, columns, ["id"],
              [{"id": 2, "payload": "cafÃ©"}, {"id": 1, "payload": text}])
              for name in migration.TABLES]
    return canonical.build("pg:1", tables)


class Publisher:
    def __init__(self, replay=False, bad=False):
        self.events = []; self.replay = replay; self.bad = bad

    def publish_immutable(self, path, body, sha256):
        self.events.append(("object", path)); table = json.loads(body)["table"]
        return {"path": path, "sha256": "0" * 64 if self.bad else sha256,
                "status": "verified-replay" if self.replay else "created", "bytes": len(body),
                "row_count": table["row_count"], "key_set_sha256": table["key_sha256"],
                "content_sha256": table["content_sha256"]}

    def publish_manifest(self, body, *, expected_head, max_attempts):
        self.events.append(("manifest", expected_head))
        return {"sha256": hashlib.sha256(body).hexdigest(),
                "status": "verified-replay" if self.replay else "committed", "cas_attempts": 1,
                "published_last": True, "head_etag": '"etag-2"'}


class Tests(unittest.TestCase):
    def test_all_seven_use_canonical_baseline_and_manifest_is_last(self):
        baseline = payload(); publisher = Publisher()
        evidence = migration.publish(baseline, expected_head="absent", publisher=publisher)
        self.assertEqual([event[0] for event in publisher.events], ["object"] * 7 + ["manifest"])
        identity = migration.verify_evidence(evidence, baseline)
        self.assertEqual(identity["encoding"], canonical.ENCODING)
        self.assertEqual(set(identity["table_identities"]), set(migration.TABLES))

    def test_unicode_uses_baseline_nfc_and_detects_mojibake(self):
        composed, decomposed = payload("café"), payload("cafe\u0301")
        self.assertEqual(composed["manifest"]["global_sha256"],
                         decomposed["manifest"]["global_sha256"])
        mojibake = payload("cafÃ©")
        self.assertNotEqual(composed["manifest"]["global_sha256"],
                            mojibake["manifest"]["global_sha256"])

    def test_replay_is_exact_and_divergent_object_stops_before_manifest(self):
        baseline = payload(); replay = Publisher(replay=True)
        migration.verify_evidence(migration.publish(baseline, expected_head='"etag-1"',
                                                     publisher=replay), baseline)
        bad = Publisher(bad=True)
        with self.assertRaisesRegex(RuntimeError, "readback verification"):
            migration.publish(baseline, expected_head="absent", publisher=bad)
        self.assertNotIn("manifest", [event[0] for event in bad.events])

    def test_wrong_baseline_and_cas_bounds_are_rejected(self):
        baseline = payload(); evidence = migration.publish(
            baseline, expected_head="absent", publisher=Publisher())
        tampered = copy.deepcopy(evidence); tampered["baseline_evidence_id"] = "c" * 64
        with self.assertRaisesRegex(RuntimeError, "not bound"):
            migration.verify_evidence(tampered, baseline)
        evidence["manifest"]["cas_attempts"] = migration.MAX_CAS_ATTEMPTS + 1
        with self.assertRaisesRegex(RuntimeError, "manifest evidence"):
            migration.verify_evidence(evidence, baseline)


if __name__ == "__main__": unittest.main()
