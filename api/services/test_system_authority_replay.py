import unittest
from datetime import datetime, timezone
from decimal import Decimal
from unittest.mock import Mock

from services import system_authority_replay as replay


class SystemAuthorityReplayTests(unittest.TestCase):
    def test_typed_values_are_lossless_and_deterministic(self):
        self.assertEqual(replay._typed(None), {"type": "null", "value": None})
        self.assertEqual(replay._typed(True), {"type": "boolean", "value": True})
        self.assertEqual(replay._typed(Decimal("1.20")), {"type": "string", "value": "1.20"})
        self.assertEqual(
            replay._typed({"z": datetime(2026, 1, 2, tzinfo=timezone.utc)}),
            {"type": "json", "value": {"z": "2026-01-02T00:00:00+00:00"}},
        )

    def test_snapshot_rejects_unregistered_identifier(self):
        with self.assertRaisesRegex(ValueError, "unsupported"):
            replay.snapshot_table("users", query=Mock())

    def test_replay_family_uses_all_tables_and_stable_keys(self):
        rows = {
            "dataset_dimensions": [{"id": 4, "dataset_id": 9, "join_condition": None}],
            "dataset_columns": [{"id": 5, "dataset_id": 9, "column_name": "region"}],
            "dataset_metrics": [{"id": 6, "dataset_id": 9, "metric_name": "count"}],
        }

        def query(sql, *_args):
            table = sql.split(" FROM ", 1)[1].split(" ORDER", 1)[0]
            return {"rows": rows[table]}

        write = Mock(return_value={"generation": 1})
        report = replay.replay_family("dataset_semantics", query=query, write=write)
        self.assertEqual(report["source_count"], 3)
        self.assertEqual(report["written"], 3)
        self.assertEqual([call.args[0] for call in write.call_args_list], [
            "dataset_dimensions", "dataset_columns", "dataset_metrics"
        ])
        self.assertEqual(write.call_args_list[0].args[1], "dataset_dimensions:4")

    def test_fallback_key_is_hash_bounded(self):
        key = replay._record_id("context_snapshots", {"element_key": "x" * 1000})
        self.assertLessEqual(len(key), 255)
        self.assertTrue(key.startswith("context_snapshots:sha256:"))


if __name__ == "__main__":
    unittest.main()
