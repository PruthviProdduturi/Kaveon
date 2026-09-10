import json
import sys
import unittest
from unittest.mock import patch
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import query_history


class QueryHistoryTests(unittest.TestCase):
    def setUp(self):
        query_history._schema_cache = None

    def test_engine_details_are_bounded_and_written_when_supported(self):
        columns = [{"column_name": name} for name in ("engine_query_id", "engine_details")]
        details = {
            "id": "engine-1",
            "state": "FAILED",
            "error": "bounded failure",
            "timings": {"planning_us": 120},
            "stages": [{"stage_id": 1, "task_count": 2}],
            "context": {"source": "studio"},
            "rows": [["must not persist"]],
            "plan": {"physical": "must not persist"},
        }
        with patch.object(query_history.db, "query", side_effect=[{"rows": columns}, {"rows": [], "row_count": 1}]) as execute:
            created = query_history.create_history({
                "sql_text": "SELECT 1", "status": "success",
                "engine_query_id": "engine-1", "engine_details": details,
            }, "analyst@example.com")
        insert_sql, params = execute.call_args_list[1].args
        self.assertIn("engine_query_id, engine_details", insert_sql)
        stored = json.loads(params[-1])
        self.assertEqual(stored["timings"], details["timings"])
        self.assertEqual(stored["state"], "FAILED")
        self.assertEqual(stored["error"], "bounded failure")
        self.assertNotIn("rows", stored)
        self.assertNotIn("plan", stored)
        self.assertEqual(created["engine_query_id"], "engine-1")

    def test_legacy_schema_keeps_existing_insert_contract(self):
        with patch.object(query_history.db, "query", side_effect=[{"rows": []}, {"rows": [], "row_count": 1}]) as execute:
            query_history.create_history({"sql_text": "SELECT 1", "status": "success"}, "analyst@example.com")
        insert_sql = execute.call_args_list[1].args[0]
        self.assertNotIn("engine_query_id, engine_details", insert_sql)

    def test_capability_probe_failure_falls_back_to_legacy_insert(self):
        with patch.object(query_history.db, "query", side_effect=[RuntimeError("denied"), {"rows": [], "row_count": 1}]) as execute:
            query_history.create_history({"sql_text": "SELECT 1", "status": "success"}, "analyst@example.com")
        self.assertNotIn("engine_query_id, engine_details", execute.call_args_list[1].args[0])

    def test_list_decodes_stored_engine_details(self):
        columns = [{"column_name": name} for name in ("engine_query_id", "engine_details")]
        rows = [{"id": "activity", "engine_details": '{"timings":{"analysis_us":5}}'}]
        with patch.object(query_history.db, "query", side_effect=[{"rows": columns}, {"rows": rows}]):
            result = query_history.list_history("analyst@example.com")
        self.assertEqual(result[0]["engine_details"]["timings"]["analysis_us"], 5)


if __name__ == "__main__":
    unittest.main()
