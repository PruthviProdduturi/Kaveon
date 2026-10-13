import hashlib
import json
import os
import tempfile
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

    def test_a_silently_occupied_key_is_reported_as_occupied(self):
        """The two artifact clients disagree about how an occupied key is
        reported: ADLS refuses the conditional write and raises, the local
        client simply does not write and returns. Only the read-back covers
        both, so an orphan left by a failed publication is recognised either
        way instead of surfacing as a bare reconciliation failure that wedges
        the dataset forever."""
        class Occupied:
            def create_if_absent(self, path, content):
                return None  # the local client's create-only behaviour

            def read(self, path, max_bytes):
                return b"bytes from an earlier attempt"

        with patch.dict(os.environ, {artifact.LIVE_PUBLISH_KEY: "true"}),              patch.object(artifact, "_client", return_value=Occupied()):
            with self.assertRaises(artifact.VersionOccupied):
                artifact.publish({**payload(), "version": 2})

    def test_a_vanished_object_is_not_mistaken_for_an_occupied_one(self):
        class Vanishing:
            def create_if_absent(self, path, content):
                return None

            def read(self, path, max_bytes):
                return None

        with patch.dict(os.environ, {artifact.LIVE_PUBLISH_KEY: "true"}),              patch.object(artifact, "_client", return_value=Vanishing()):
            with self.assertRaisesRegex(RuntimeError, "vanished"):
                artifact.publish({**payload(), "version": 2})

    def test_absent_definition_is_distinguishable_so_generation_can_start(self):
        """A dataset with no compiled definition must raise the specific
        DefinitionUnavailable, not a bare RuntimeError. Generation pre-reads the
        artifact to carry prior curation forward; if it cannot tell "never
        compiled" from "something is broken" it refuses to compile, and the
        first DLM for every dataset becomes impossible to build."""
        dataset = {"revision": 1, "document": {"created_by": "owner", "visibility": "published"}}
        with patch.object(artifact.product_store, "read", side_effect=[dataset, None]):
            with self.assertRaises(artifact.DefinitionUnavailable):
                artifact.read("7", "owner", "Admin")
        # Serving callers treat this module as fail-closed, so it stays a
        # RuntimeError for everyone who does not ask for the distinction.
        self.assertTrue(issubclass(artifact.DefinitionUnavailable, RuntimeError))

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
        read.assert_called_once_with("7", "viewer", "Viewer", context=None)
        postgres.assert_not_called()


if __name__ == "__main__": unittest.main()


class TheStorageClientIsKeptRatherThanRebuilt(unittest.TestCase):
    """A fresh client means a fresh credential, and so an empty token cache.

    Measured in the cluster: 0.24s to obtain a storage token against 0.045s to
    read the object it protects. Built per read, nine datasets on the home page
    spent two seconds proving who we are to a service we had just finished
    talking to.
    """

    def setUp(self):
        artifact.reset_client()
        self.addCleanup(artifact.reset_client)

    def test_repeated_reads_share_one_client(self):
        with patch.dict(os.environ, {"KAVEON_LOCAL_PRODUCT_MODE": ""}, clear=False),              patch.object(artifact.adls_artifact_client.AzureArtifactClient,
                          "from_env", return_value=object()) as built:
            first = artifact._client()
            second = artifact._client()
        self.assertIs(first, second)
        built.assert_called_once()

    def test_the_local_mode_client_is_not_shared_into_the_cloud_path(self):
        # Local mode addresses a caller-owned directory, so it is built per
        # call and must never be the object a cloud read gets handed.
        with patch.dict(os.environ, {"KAVEON_LOCAL_PRODUCT_MODE": "true",
                                     "KAVEON_LOCAL_DLM_ARTIFACT_PATH": self.enterContext(
                                         tempfile.TemporaryDirectory())}):
            local = artifact._client()
        self.assertIsInstance(local, artifact.LocalArtifactClient)
        with patch.dict(os.environ, {"KAVEON_LOCAL_PRODUCT_MODE": ""}, clear=False),              patch.object(artifact.adls_artifact_client.AzureArtifactClient,
                          "from_env", return_value=object()):
            self.assertIsNot(artifact._client(), local)


class ListingsForOneBatchHappenTogether(unittest.TestCase):
    """The three catalog listings are independent and cost the same each.

    Each costs about half a second of catalog round trips however few records
    it returns, so taken in turn they spent a second and a half waiting rather
    than working. Nothing in the second depends on the first.
    """

    def test_the_three_families_are_listed_concurrently(self):
        import threading
        started = threading.Barrier(3, timeout=5)
        seen = []

        def _list(family, actor, role, *, max_records=1000):
            seen.append(family)
            # A sequential implementation deadlocks here.
            started.wait()
            return []

        with patch.object(artifact.product_store, "list_records",
                          side_effect=_list):
            context = artifact.load_bulk_context("owner")
        self.assertEqual(sorted(seen), ["dataset", "dlm_definition", "dlm_run"])
        self.assertEqual((context.datasets, context.definitions, context.runs),
                         ({}, {}, []))

    def test_each_family_lands_in_its_own_field(self):
        def _list(family, actor, role, *, max_records=1000):
            return [{"id": family, "revision": 1, "document": {}}]

        with patch.object(artifact.product_store, "list_records",
                          side_effect=_list):
            context = artifact.load_bulk_context("owner")
        # Concurrency must not shuffle which listing is which.
        self.assertEqual(list(context.datasets), ["dataset"])
        self.assertEqual(list(context.definitions), ["dlm_definition"])
        self.assertEqual([r["id"] for r in context.runs], ["dlm_run"])


class TheCacheKeyTracksCompiledContentOnly(unittest.TestCase):
    """What the key must and must not move for.

    A key taken from the catalog's own snapshot identity would move on every
    product write — and the platform records its query history in that same
    catalog, so running one query would invalidate a banner that could not
    have changed.
    """

    def _context(self, dataset_revision=1, definition_revision=1, run_status="ready"):
        return artifact.BulkContext(
            {"7": {"id": "7", "revision": dataset_revision}},
            {"7": {"id": "7", "revision": definition_revision}},
            [{"id": "7-v1", "revision": 1, "document": {"status": run_status}}],
        )

    def test_the_same_records_give_the_same_key(self):
        self.assertEqual(artifact.context_identity(self._context()),
                         artifact.context_identity(self._context()))

    def test_a_new_dataset_revision_moves_the_key(self):
        self.assertNotEqual(
            artifact.context_identity(self._context()),
            artifact.context_identity(self._context(dataset_revision=2)))

    def test_a_new_definition_revision_moves_the_key(self):
        self.assertNotEqual(
            artifact.context_identity(self._context()),
            artifact.context_identity(self._context(definition_revision=2)))

    def test_a_run_changing_state_moves_the_key(self):
        self.assertNotEqual(
            artifact.context_identity(self._context()),
            artifact.context_identity(self._context(run_status="failed")))

    def test_a_new_run_moves_the_key(self):
        later = self._context()
        later.runs.append({"id": "7-v2", "revision": 1, "document": {"status": "ready"}})
        self.assertNotEqual(artifact.context_identity(self._context()),
                            artifact.context_identity(later))
