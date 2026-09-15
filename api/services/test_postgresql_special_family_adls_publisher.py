import unittest

from services import postgresql_baseline_identity as canonical
from services import postgresql_special_family_adls_publisher as adapter
from services import postgresql_special_family_migration as migration


class Conflict(Exception):
    status = 412


class Client:
    def __init__(self):
        self.values = {}; self.etags = {}; self.sequence = 0; self.puts = []

    def _etag(self):
        self.sequence += 1; return f'"etag-{self.sequence}"'

    def create_if_absent_with_etag(self, path, body):
        if path in self.values: raise Conflict()
        self.values[path] = body; self.etags[path] = self._etag(); return self.etags[path]

    def read(self, path, max_bytes): return self.values.get(path)

    def read_with_etag(self, path, max_bytes):
        return ((self.values[path], self.etags[path]) if path in self.values else None)

    def put_if_match(self, path, body, expected):
        self.puts.append((path, expected))
        if ((expected is None and path in self.values) or
                (expected is not None and self.etags.get(path) != expected)):
            raise Conflict()
        self.values[path] = body; self.etags[path] = self._etag(); return self.etags[path]


def payload():
    columns = [{"name": "id", "type": "integer", "nullable": False, "ordinal": 1}]
    tables = [canonical.table_identity(name, columns, ["id"], [{"id": 1}])
              for name in migration.TABLES if name != "dlm_artifact"]
    artifact_columns = [
        {"name": "dataset_id", "type": "text", "nullable": False, "ordinal": 1},
        {"name": "manifest", "type": "jsonb", "nullable": False, "ordinal": 2}]
    tables.append(canonical.table_identity("dlm_artifact", artifact_columns, ["dataset_id"],
        [{"dataset_id": "17", "manifest": {"name": "Climate × Energy"}}]))
    return canonical.build("pg:qualified", tables)


class Tests(unittest.TestCase):
    def test_create_readback_manifest_last_and_exact_replay(self):
        client = Client(); publisher = adapter.Publisher(client, "retirement/run-1")
        first = migration.publish(payload(), expected_head="absent", publisher=publisher, source_pending_events=0)
        self.assertEqual(first["manifest"]["status"], "committed")
        self.assertEqual(len(client.puts), 1)
        second = migration.publish(payload(), expected_head='"stale-is-irrelevant-for-replay"',
                                   publisher=publisher, source_pending_events=0)
        self.assertEqual(second["manifest"]["status"], "verified-replay")
        self.assertTrue(all(item["status"] == "verified-replay" for item in second["objects"]))

    def test_divergent_immutable_object_and_stale_head_fail(self):
        client = Client(); publisher = adapter.Publisher(client, "retirement/run-1")
        identity = migration.verify_baseline(payload())
        path = f"retirement/run-1/objects/{identity['baseline_evidence_id']}/{migration.TABLES[0]}.json"
        client.values[path] = b"wrong"; client.etags[path] = '"etag-old"'
        with self.assertRaisesRegex(RuntimeError, "readback mismatch"):
            migration.publish(payload(), expected_head="absent", publisher=publisher, source_pending_events=0)
        client = Client(); publisher = adapter.Publisher(client, "retirement/run-1")
        client.values["retirement/run-1/head.json"] = b"other"
        client.etags["retirement/run-1/head.json"] = '"etag-current"'
        with self.assertRaisesRegex(RuntimeError, "changed concurrently"):
            migration.publish(payload(), expected_head='"etag-stale"', publisher=publisher, source_pending_events=0)

    def test_ambiguous_head_success_is_verified_by_readback(self):
        class Ambiguous(Client):
            def put_if_match(self, path, body, expected):
                super().put_if_match(path, body, expected)
                raise OSError("response lost")
        evidence = migration.publish(payload(), expected_head="absent",
            publisher=adapter.Publisher(Ambiguous(), "retirement/run-1"), source_pending_events=0)
        self.assertEqual(evidence["manifest"]["status"], "verified-replay")


if __name__ == "__main__": unittest.main()
