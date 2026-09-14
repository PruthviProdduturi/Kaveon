import hashlib
import json
import os
import unittest
from unittest.mock import Mock, patch

from services import dlm_compiled_artifact as artifact


def payload(version=2):
    return {"dataset_id": "7", "version": version, "manifest": {"name": "orders"},
            "stats_rollup": {"rows": 12}, "usage_rollup": {}, "source_hash": "source",
            "built_at": "2026-09-14T20:00:00Z", "status": "ready", "values_indexed": 3}


class Client:
    def __init__(self): self.values = {}
    def create_if_absent(self, path, content):
        if path in self.values: raise RuntimeError("exists")
        self.values[path] = content
    def read(self, path, max_bytes): return self.values.get(path)


class CompiledArtifactTests(unittest.TestCase):
    def test_publish_is_opt_in_create_only_and_exact(self):
        client = Client()
        with patch.dict(os.environ, {}, clear=True):
            self.assertIsNone(artifact.publish(payload()))
        with patch.dict(os.environ, {artifact.LIVE_PUBLISH_KEY: "true"}, clear=True), \
             patch.object(artifact, "_client", return_value=client):
            result = artifact.publish(payload())
            repeated = artifact.publish(payload())
        self.assertEqual(result["sha256"], repeated["sha256"])
        self.assertEqual(result["path"], "dlm/7/v2/compiled.json")

    def test_retirement_context_shape_is_strict_and_immutable(self):
        complete = {**payload(), "compiled_context": {
            "values": [], "answers": [], "sketches": [], "router": {}, "curation": {},
        }}
        client = Client()
        with patch.dict(os.environ, {artifact.LIVE_PUBLISH_KEY: "true"}, clear=True), \
             patch.object(artifact, "_client", return_value=client):
            self.assertEqual(artifact.publish(complete)["version"], 2)
            with self.assertRaisesRegex(RuntimeError, "context payload"):
                artifact.publish({**payload(3), "compiled_context": {"values": []}})

    def test_read_binds_dataset_definition_run_and_bytes(self):
        client = Client(); content = artifact._canonical(payload())
        path = "dlm/7/v2/compiled.json"; client.values[path] = content
        digest = hashlib.sha256(content).hexdigest()
        dataset = {"revision": 4, "document": {"created_by": "owner", "visibility": "published"}}
        definition = {"revision": 3, "document": {"dataset_id": "7", "dataset_revision": 4}}
        runs = [{"id": "7-v2", "document": {"definition_id": "7", "definition_revision": 3,
                 "status": "ready", "artifact": {"path": path, "sha256": digest}}}]
        with patch.object(artifact.product_store, "read", side_effect=[dataset, definition]), \
             patch.object(artifact.product_store, "list_records", return_value=runs), \
             patch.object(artifact, "_client", return_value=client):
            self.assertEqual(artifact.read("7", "viewer", "Viewer"), payload())

    def test_read_fails_closed_on_stale_definition_or_corrupt_bytes(self):
        dataset = {"revision": 4, "document": {"created_by": "owner", "visibility": "published"}}
        stale = {"revision": 3, "document": {"dataset_id": "7", "dataset_revision": 2}}
        with patch.object(artifact.product_store, "read", side_effect=[dataset, stale]), \
             self.assertRaisesRegex(RuntimeError, "stale"):
            artifact.read("7", "viewer", "Viewer")
        content = artifact._canonical(payload()); path = "dlm/7/v2/compiled.json"
        definition = {"revision": 3, "document": {"dataset_id": "7", "dataset_revision": 4}}
        runs = [{"id": "7-v2", "document": {"definition_id": "7", "definition_revision": 3,
                 "status": "ready", "artifact": {"path": path, "sha256": "a" * 64}}}]
        client = Client(); client.values[path] = content
        with patch.object(artifact.product_store, "read", side_effect=[dataset, definition]), \
             patch.object(artifact.product_store, "list_records", return_value=runs), \
             patch.object(artifact, "_client", return_value=client), \
            self.assertRaisesRegex(RuntimeError, "corrupt"):
            artifact.read("7", "viewer", "Viewer")

    def test_read_applies_visibility_after_server_asserted_admin_read(self):
        private = {"revision": 1, "document": {"created_by": "owner", "visibility": "private"}}
        with patch.object(artifact.product_store, "read", return_value=private) as read:
            self.assertIsNone(artifact.read("7", "other", "Viewer"))
        read.assert_called_once_with("dataset", "7", "other", "Admin")
        internal = {"revision": 1, "document": {"created_by": "owner", "visibility": "internal"}}
        with patch.object(artifact.product_store, "read", side_effect=[internal, None]) as read, \
             self.assertRaisesRegex(RuntimeError, "missing or stale"):
            artifact.read("7", "analyst", "Analyst")
        self.assertTrue(all(call.args[-1] == "Admin" for call in read.call_args_list))

    def test_enqueue_run_is_owner_and_definition_revision_bound(self):
        transaction = Mock()
        with patch.object(artifact.product_outbox, "enqueue") as enqueue:
            artifact.enqueue_run(transaction, "7", "owner", "actor", 5,
                {"version": 2, "path": "dlm/7/v2/compiled.json", "sha256": "a" * 64})
        self.assertEqual(enqueue.call_args.kwargs["owner"], "owner")
        self.assertEqual(enqueue.call_args.kwargs["payload"]["definition_revision"], 5)

    def test_engine_retirement_read_has_no_postgresql_fallback(self):
        from dlm import engine
        expected = payload()
        with patch.object(engine.meta, "query_one") as postgres, \
             patch("services.postgresql_retirement_runtime.requested", return_value=True), \
             patch.object(artifact, "read", return_value=expected) as read:
            self.assertIs(engine.get_dlm("7", "viewer", "Viewer"), expected)
        read.assert_called_once_with("7", "viewer", "Viewer")
        postgres.assert_not_called()


if __name__ == "__main__": unittest.main()
