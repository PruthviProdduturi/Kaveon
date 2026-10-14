"""Discovery stays inside registered sources, and refuses to guess a name.

The two behaviours worth pinning are the ones that caused trouble before:
auto-registration that swept benchmark scratch into the product catalog, and a
mapping from a lake path to `catalog.schema.table` that cannot always be
decided. Everything else here is bookkeeping.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import lake_discovery as discovery


def source(name, account="kaveonlake", container="telemetry", root="events",
           active=True, lifecycle="active", storage=None):
    document = {
        "name": name, "catalog_identity": name, "is_active": active,
        "lifecycle": lifecycle,
        "storage_config": {"account": account, "container": container, "root_path": root}
        if storage is None else storage,
    }
    return {"id": name, "revision": 1, "document": document}


class FakeClient:
    """Returns blob paths the way the real listing does — flat, with the Delta
    log's own objects present, which is how a table directory is recognised."""

    def __init__(self, paths, fail=False):
        self.paths = paths
        self.fail = fail

    def list(self, prefix, max_objects=10000):
        if self.fail:
            raise RuntimeError("ADLS list exceeds its object bound")
        return [{"path": p, "etag": "e", "size": 1} for p in self.paths]


class ScopeTests(unittest.TestCase):
    def test_only_active_lake_backed_sources_are_scanned(self):
        records = [
            source("KaveonTelemetry"),
            source("Retired", lifecycle="retired"),
            source("Disabled", active=False),
            # The product catalog carries a base_path rather than a lake prefix.
            source("KaveonDB", storage={"base_path": "/data"}),
        ]
        with patch("services.product_store.list_records", return_value=records):
            names = [s["catalog"] for s in discovery.ingestible_sources("a@b.c")]
        self.assertEqual(names, ["KaveonTelemetry"])

    def test_a_source_with_no_root_is_not_scanned(self):
        """Without a root there is no prefix to confine the scan to, and
        scanning a whole container is how scratch gets swept in."""
        records = [source("Loose", root="")]
        with patch("services.product_store.list_records", return_value=records):
            self.assertEqual(discovery.ingestible_sources("a@b.c"), [])


class DirectoryTests(unittest.TestCase):
    def test_a_delta_log_marks_its_directory_as_a_table(self):
        client = FakeClient([
            "events/enriched/_delta_log/00000000000000000000.json",
            "events/enriched/part-0.parquet",
            "events/daily_user_activity/_delta_log/00000000000000000000.json",
            "events/daily_user_activity/part-0.parquet",
            # Parquet with no log is not a table: nothing establishes a commit.
            "events/loose_parquet/part-0.parquet",
        ])
        self.assertEqual(discovery.table_directories(client, "events"),
                         ["daily_user_activity", "enriched"])

    def test_a_nested_table_keeps_its_relative_path(self):
        client = FakeClient([
            "snapshots/v1/public/kaveon_events/_delta_log/0.json",
        ])
        self.assertEqual(discovery.table_directories(client, "snapshots/v1"),
                         ["public/kaveon_events"])

    def test_a_root_too_large_to_list_reports_rather_than_raises(self):
        """A scheduled sweep must not be taken down by the size of a prefix."""
        self.assertIsNone(discovery.table_directories(FakeClient([], fail=True), "events"))


class PlacementTests(unittest.TestCase):
    def test_two_segments_take_the_schema_from_the_path(self):
        schema, table, _ = discovery.placement("public/kaveon_events", ["public", "covid"])
        self.assertEqual((schema, table), ("public", "kaveon_events"))

    def test_one_segment_is_decidable_only_with_a_single_schema(self):
        schema, table, _ = discovery.placement("daily_user_activity", ["events"])
        self.assertEqual((schema, table), ("events", "daily_user_activity"))

    def test_one_segment_with_several_schemas_is_declined_and_says_why(self):
        schema, table, reason = discovery.placement(
            "orphan", ["public", "covid", "ai_benchmarks"])
        self.assertIsNone(schema)
        self.assertIsNone(table)
        self.assertIn("needs a schema", reason)
        self.assertIn("public", reason)

    def test_a_deeper_path_is_declined(self):
        schema, _, reason = discovery.placement("a/b/c", ["public"])
        self.assertIsNone(schema)
        self.assertIn("3-segment", reason)


class DiscoverTests(unittest.TestCase):
    def _run(self, *, register, directories, known, schemas=("events",)):
        with patch("services.product_store.list_records",
                   return_value=[source("KaveonTelemetry")]), \
             patch("services.adls_artifact_client.AzureArtifactClient",
                   return_value=FakeClient(directories)), \
             patch.object(discovery, "registered_locations", return_value=set(known)), \
             patch("services.engine_bridge.schemas",
                   return_value={"schemas": [{"name": s} for s in schemas]}), \
             patch("services.engine_bridge.create_table_inferred",
                   return_value={"ok": True}) as create:
            report = discovery.discover("a@b.c", register=register)
        return report, create

    def test_an_already_registered_location_is_left_alone(self):
        report, create = self._run(
            register=True,
            directories=["events/enriched/_delta_log/0.json"],
            known={"enriched"})
        self.assertEqual(report["registered"], [])
        self.assertEqual(report["found"], [])
        create.assert_not_called()

    def test_a_new_table_is_reported_but_not_registered_by_default(self):
        """The reading is useful on its own; a sweep that writes to the catalog
        should have been asked to."""
        report, create = self._run(
            register=False,
            directories=["events/daily_user_activity/_delta_log/0.json"],
            known={"enriched"})
        self.assertEqual([f["location"] for f in report["found"]], ["daily_user_activity"])
        self.assertEqual(report["registered"], [])
        create.assert_not_called()

    def test_a_new_table_is_registered_when_asked(self):
        report, create = self._run(
            register=True,
            directories=["events/daily_user_activity/_delta_log/0.json"],
            known={"enriched"})
        self.assertEqual(len(report["registered"]), 1)
        entry = report["registered"][0]
        self.assertEqual((entry["schema"], entry["table"]), ("events", "daily_user_activity"))
        args = create.call_args.args
        self.assertEqual(args[:5], ("KaveonTelemetry", "events", "daily_user_activity",
                                    "daily_user_activity", "Delta"))

    def test_an_unreadable_catalog_registers_nothing(self):
        """`None` from the catalog read means "could not be read", which must
        never be treated as "holds nothing" — that would register every table
        in the lake a second time."""
        with patch("services.product_store.list_records",
                   return_value=[source("KaveonTelemetry")]), \
             patch("services.adls_artifact_client.AzureArtifactClient",
                   return_value=FakeClient(["events/x/_delta_log/0.json"])), \
             patch.object(discovery, "registered_locations", return_value=None), \
             patch("services.engine_bridge.create_table_inferred") as create:
            report = discovery.discover("a@b.c", register=True)
        create.assert_not_called()
        self.assertEqual(len(report["unreadable"]), 1)

    def test_the_registration_bound_holds(self):
        directories = [f"events/t{i}/_delta_log/0.json" for i in range(5)]
        with patch("services.product_store.list_records",
                   return_value=[source("KaveonTelemetry")]), \
             patch("services.adls_artifact_client.AzureArtifactClient",
                   return_value=FakeClient(directories)), \
             patch.object(discovery, "registered_locations", return_value=set()), \
             patch("services.engine_bridge.schemas",
                   return_value={"schemas": [{"name": "events"}]}), \
             patch("services.engine_bridge.create_table_inferred",
                   return_value={"ok": True}):
            report = discovery.discover("a@b.c", register=True, limit=2)
        self.assertEqual(len(report["registered"]), 2)
        self.assertEqual(len(report["undecidable"]), 3)
        self.assertIn("bound of 2", report["undecidable"][0]["reason"])

    def test_a_refused_registration_is_reported_and_does_not_stop_the_run(self):
        directories = ["events/a/_delta_log/0.json", "events/b/_delta_log/0.json"]
        with patch("services.product_store.list_records",
                   return_value=[source("KaveonTelemetry")]), \
             patch("services.adls_artifact_client.AzureArtifactClient",
                   return_value=FakeClient(directories)), \
             patch.object(discovery, "registered_locations", return_value=set()), \
             patch("services.engine_bridge.schemas",
                   return_value={"schemas": [{"name": "events"}]}), \
             patch("services.engine_bridge.create_table_inferred",
                   side_effect=[{"ok": False, "message": "unreadable location"},
                                {"ok": True}]):
            report = discovery.discover("a@b.c", register=True)
        self.assertEqual(len(report["registered"]), 1)
        self.assertEqual(len(report["undecidable"]), 1)
        self.assertIn("refused", report["undecidable"][0]["reason"])


if __name__ == "__main__":
    unittest.main()
