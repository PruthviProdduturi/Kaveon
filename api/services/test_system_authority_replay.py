import unittest
from datetime import datetime, timezone
from decimal import Decimal
from unittest.mock import Mock, patch

from fastapi import HTTPException

from services import engine_system_store
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
        report = replay.replay_family("dataset_semantics", query=query, write=write,
                                      read=lambda *_args, **_kwargs: None)
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

    def test_replay_is_safe_to_resume_and_updates_divergent_target(self):
        def query(sql, *_args):
            return {"rows": [{"id": 8, "name": "current"}]}

        columns = {"id": {"type": "integer", "value": 8},
                   "name": {"type": "string", "value": "current"}}
        read = Mock(side_effect=[{"columns": columns, "revision": 3},
                                 {"columns": {**columns, "name": {"type": "string", "value": "old"}}, "revision": 2}])
        update = Mock()
        first = replay.replay_table("catalog_sources", query=query, read=read, update=update,
                                    write=Mock(), actor="migration")
        self.assertEqual(first["skipped"], 1)
        self.assertEqual(first["updated"], 0)
        second = replay.replay_table("catalog_sources", query=query, read=read, update=update,
                                     write=Mock(), actor="migration")
        self.assertEqual(second["updated"], 1)
        update.assert_called_once_with("catalog_sources", "catalog_sources:8", columns, 2, "migration", "Admin")


class BatchPathReportingTests(unittest.TestCase):
    """What the replay report says about a large table.

    `source_count` used to be `len(rows)` read at the END of the function, and
    the batch path rebinds `rows` to the handful needing row-by-row
    reconciliation — filtering on a `target_cache` it had just emptied, so the
    list was always empty. A replay of half a million rows reported
    `source_count: 0, written: 500000`, and a retirement gate's parity
    predicate is `source_count == target_count`, which `0 == 0` satisfies.
    """

    def _rows(self, count):
        return [{"id": index, "user_email": "a@b.c", "action": "opened",
                 "object_type": "chart", "object_id": "c-1", "object_name": "C",
                 "timestamp": "2026-01-01T00:00:00", "details": None}
                for index in range(1, count + 1)]

    def _replay(self, rows, *, list_rows):
        """Run a replay down the batch path, returning the report and the
        batch mutation so a test can pin which path it took."""
        with patch.object(engine_system_store, "list_rows", side_effect=list_rows), \
             patch.object(engine_system_store, "create_rows",
                          return_value={"generation": 1}) as batched, \
             patch.object(engine_system_store, "create_row",
                          return_value={"generation": 1}), \
             patch.object(engine_system_store, "read_row", return_value=None):
            report = replay.replay_table(
                "activity", "migration", query=lambda *_a, **_k: {"rows": rows},
                write=engine_system_store.create_row,
                read=engine_system_store.read_row)
        return report, batched

    def test_a_large_table_reports_the_rows_the_source_held(self):
        rows = self._rows(1200)
        report, batched = self._replay(rows, list_rows=lambda *a, **k: {"rows": []})
        self.assertTrue(batched.called, "this must exercise the batch path")
        self.assertEqual(report["source_count"], 1200)
        self.assertEqual(report["written"] + report["skipped"] + report["updated"],
                         1200)

    def test_an_absent_typed_table_is_still_treated_as_an_empty_target(self):
        """A first replay: the Engine answers 404 and `list_rows` reports it
        as an invalid page. That one message still means an empty target."""
        rows = self._rows(5)
        report, _ = self._replay(rows, list_rows=HTTPException(
            502, replay._ABSENT_TARGET_PAGE))
        self.assertEqual(report["source_count"], 5)

    def test_a_transport_or_credential_failure_is_not_an_empty_target(self):
        """Every one of these is also a 502 from the bridge, so the bare
        status check this replaces swallowed all of them into an empty target
        and forced the blind batch-create path."""
        for detail in ("Engine is unavailable",
                       "Engine rejected the bridge credential",
                       "Engine rejected the request (upstream HTTP 500)",
                       "KaveonDB system row page omitted its snapshot identity"):
            with self.subTest(detail=detail):
                with self.assertRaises(HTTPException) as caught:
                    self._replay(self._rows(5),
                                 list_rows=HTTPException(502, detail))
                self.assertEqual(str(caught.exception.detail), detail)


if __name__ == "__main__":
    unittest.main()
