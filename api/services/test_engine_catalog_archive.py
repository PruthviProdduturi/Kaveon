import unittest
from unittest.mock import patch

from services import engine_catalog_archive as archive


class FakeClient:
    """A create-only object store, with the one behaviour that matters here:
    an existing path is never overwritten, and reads return exactly what was
    written."""

    def __init__(self, seed=None):
        self.objects = dict(seed or {})
        self.writes = []

    def read(self, path, max_bytes):
        value = self.objects.get(path)
        return None if value is None else value[:max_bytes]

    def create_if_absent_with_etag(self, path, content):
        if path in self.objects:
            raise AssertionError(f"create-only store was asked to overwrite {path}")
        self.objects[path] = content
        self.writes.append(path)
        return f'"etag-{len(self.objects)}"'


def registry(*, version=0, observed_at=1_000, table="energy"):
    """One catalog, one schema, one table, with statistics that report a source
    version — the shape the Engine's catalog API actually returns."""
    return {
        "/v1/catalog/definitions": [
            {"id": "platform-1", "name": "OpenSource", "revision": 2, "lifecycle": "Active",
             "storage": {"AdlsGen2": {"account": "lake", "container": "opensource",
                                      "root_path": "snapshots/v1"}}},
        ],
        "/v1/catalog/definitions/platform-1/schemas": [
            {"id": "platform-1-climate", "catalog_id": "platform-1", "name": "climate",
             "revision": 2, "lifecycle": "Active"},
        ],
        "/v1/catalog/schemas/platform-1-climate/tables": [
            {"id": f"table:OpenSource:climate:{table}", "schema_id": "platform-1-climate",
             "name": table, "revision": 2, "location": f"climate/{table}",
             "access": "Shortcut", "format": "Delta",
             "columns": [{"name": "country", "data_type": "Utf8", "nullable": True}]},
        ],
        f"/v1/catalog/tables/table%3AOpenSource%3Aclimate%3A{table}/statistics": {
            "observed_at_ms": observed_at,
            "source_version": {"identity_sha256": "a" * 64, "kind": "delta_version",
                               "version": version},
            "stale": False,
            "statistics": {"bytes": 97765, "rows": 1234},
        },
    }


def reader(responses):
    def read(path, actor, **kwargs):
        if path not in responses:
            raise AssertionError(f"unexpected catalog path {path}")
        return responses[path]
    return read


class EngineCatalogArchiveTests(unittest.TestCase):
    def test_export_walks_the_whole_registry(self):
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry())):
            document = archive.export()
        self.assertEqual(document["counts"], {"catalogs": 1, "schemas": 1, "tables": 1,
                                              "tables_with_statistics": 1})
        table = document["catalogs"][0]["schemas"][0]["tables"][0]["table"]
        # The irreplaceable part of a registration is where the table lives and
        # how to read it; a restore that loses this cannot find the data.
        self.assertEqual((table["location"], table["format"], table["access"]),
                         ("climate/energy", "Delta", "Shortcut"))

    def test_re_observing_unchanged_statistics_is_not_a_change(self):
        """Statistics carry the wall-clock time they were observed. If that
        counted as a change, every scheduled run would archive an identical
        registry and the container would grow without bound."""
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(observed_at=1))):
            first = archive.identity(archive.export())
        with patch.object(archive.engine_bridge, "catalog_read",
                          reader(registry(observed_at=9_999_999))):
            second = archive.identity(archive.export())
        self.assertEqual(first, second)

    def test_a_new_table_version_is_a_change(self):
        """The guard above must not blind the archive to the thing it exists to
        capture: a table that actually advanced."""
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(version=0))):
            first = archive.identity(archive.export())
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(version=7))):
            second = archive.identity(archive.export())
        self.assertNotEqual(first, second)

    def test_a_new_registry_is_published_with_a_run_record(self):
        client = FakeClient()
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry())):
            result = archive.archive(client, now_ms=1_700_000_000_000)
        self.assertTrue(result["written"])
        self.assertEqual(client.writes,
                         [f"engine-catalog/catalog/v1/{result['identity_sha256']}.json",
                          f"engine-catalog/runs/v1/{result['run_id']}.json"])

    def test_an_unchanged_registry_writes_nothing(self):
        """The architect's rule: object storage sees a write when something
        changed and not otherwise."""
        client = FakeClient()
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry())):
            first = archive.archive(client, now_ms=1_700_000_000_000)
            before = dict(client.objects)
            second = archive.archive(client, now_ms=1_700_000_600_000)
        self.assertTrue(first["written"])
        self.assertFalse(second["written"])
        self.assertIsNone(second["run_id"])
        self.assertEqual(client.objects, before)
        self.assertEqual(second["identity_sha256"], first["identity_sha256"])
        # An unchanged registry is still a successful run that knows what it saw.
        self.assertEqual(second["counts"], first["counts"])

    def test_a_second_run_at_a_later_observation_time_is_still_unchanged(self):
        """The case the unchanged-registry test above could not see.

        That test archives the same registry twice with the same observation
        timestamp, so the bytes matched by luck. In production every run
        observes statistics afresh, so `observed_at_ms` moves while the
        identity does not — and the archive wrote a payload that its own
        content-addressed path did not describe. The corruption check then
        fired on the ordinary case and the registry stopped being archived at
        all, which is how this reached the cluster.
        """
        client = FakeClient()
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(observed_at=1))):
            first = archive.archive(client, now_ms=1_700_000_000_000)
        before = dict(client.objects)
        with patch.object(archive.engine_bridge, "catalog_read",
                          reader(registry(observed_at=9_999_999))):
            second = archive.archive(client, now_ms=1_700_000_600_000)
        self.assertTrue(first["written"])
        self.assertFalse(second["written"])
        self.assertEqual(client.objects, before)

    def test_the_stored_bytes_are_the_bytes_the_path_is_named_for(self):
        """A content-addressed path whose digest does not cover its own content
        is not content-addressed, and makes every integrity check meaningless."""
        import hashlib
        client = FakeClient()
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry())):
            result = archive.archive(client, now_ms=1_700_000_000_000)
        stored = client.objects[result["path"]]
        self.assertEqual(hashlib.sha256(stored).hexdigest(), result["identity_sha256"])
        self.assertTrue(result["path"].endswith(f"{result['identity_sha256']}.json"))

    def test_a_table_that_advanced_is_archived_beside_the_old_one(self):
        client = FakeClient()
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(version=0))):
            first = archive.archive(client, now_ms=1_700_000_000_000)
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry(version=7))):
            second = archive.archive(client, now_ms=1_700_000_600_000)
        self.assertTrue(second["written"])
        self.assertNotEqual(second["path"], first["path"])
        # History is kept: the earlier registry is still restorable.
        self.assertIn(first["path"], client.objects)

    def test_corruption_at_a_content_addressed_path_is_not_reported_as_unchanged(self):
        """A path named for a digest that holds other bytes is corruption.
        Treating it as 'already archived' would leave the registry unarchived
        and say the run succeeded."""
        with patch.object(archive.engine_bridge, "catalog_read", reader(registry())):
            document = archive.export()
            key = archive.identity(document)
            client = FakeClient({f"engine-catalog/catalog/v1/{key}.json": b'{"not":"the registry"}'})
            with self.assertRaises(RuntimeError):
                archive.archive(client)

    def test_an_empty_registry_is_refused(self):
        """An Engine that answers with no catalogs has nothing worth archiving,
        and publishing it would let an empty archive become the newest one."""
        with patch.object(archive.engine_bridge, "catalog_read",
                          reader({"/v1/catalog/definitions": []})):
            with self.assertRaises(RuntimeError):
                archive.export()

    def test_a_partial_answer_fails_rather_than_archiving_a_gap(self):
        """A registry missing tables restores a catalog that cannot find its
        data, and does it silently. Better to fail the run."""
        responses = registry()
        responses["/v1/catalog/schemas/platform-1-climate/tables"] = None
        with patch.object(archive.engine_bridge, "catalog_read", reader(responses)):
            with self.assertRaises(RuntimeError):
                archive.export()

    def test_a_table_with_no_statistics_is_still_archived(self):
        """Statistics are derivable; a registration is not. A table nobody has
        analysed must not keep the registry from being archived."""
        responses = registry()
        responses["/v1/catalog/tables/table%3AOpenSource%3Aclimate%3Aenergy/statistics"] = None
        with patch.object(archive.engine_bridge, "catalog_read", reader(responses)):
            document = archive.export()
        self.assertEqual(document["counts"]["tables"], 1)
        self.assertEqual(document["counts"]["tables_with_statistics"], 0)

    def test_the_archive_container_defaults_away_from_the_live_catalog(self):
        with patch.dict("os.environ", {}, clear=True):
            self.assertEqual(archive.container(), "backups")
        with patch.dict("os.environ", {"KAVEON_CATALOG_ARCHIVE_CONTAINER": "vault"}, clear=True):
            self.assertEqual(archive.container(), "vault")


if __name__ == "__main__":
    unittest.main()
