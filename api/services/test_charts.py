import json
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import charts


class ChartPostgresSchemaTests(unittest.TestCase):
    def setUp(self):
        self.schema = patch.object(charts, "_chart_schema", return_value="modern")
        self.schema.start()

    def tearDown(self):
        self.schema.stop()

    def test_list_uses_dataset_id_and_config_not_legacy_json_columns(self):
        row = {
            "id": "chart-1", "name": "Trips", "dataset_id": 7, "chart_type": "bar",
            "config": json.dumps({"query_config": {"dataset_id": 7, "groupby": ["borough"]},
                                  "viz_config": {"chartType": "bar"}}),
            "visibility": "published", "created_by": "seed@example.com",
        }
        with patch.object(charts.db, "query", return_value={"rows": [row]}) as query:
            items = charts.list_charts("viewer@example.com", "Viewer")
        statement = query.call_args.args[0]
        self.assertIn("ds.id = c.dataset_id", statement)
        self.assertIn("c.config", statement)
        self.assertNotIn("query_config", statement)
        self.assertEqual(items[0]["dataset_id"], "7")
        self.assertEqual(items[0]["query_config"]["groupby"], ["borough"])

    def test_authenticated_point_read_observes_shadow_without_changing_response(self):
        row = {
            "id": "chart-1", "name": "Trips", "dataset_id": 7, "chart_type": "bar",
            "config": "{}", "visibility": "private", "created_by": "owner@example.test",
        }
        with patch.object(charts.db, "query_one", return_value=row), \
             patch.object(charts.product_shadow_read, "compare_chart", return_value={"enabled": True, "status": "mismatch"}) as compare:
            result = charts.get_chart_by_id("chart-1", "owner@example.test", "Viewer")
        compare.assert_called_once_with(result, "owner@example.test", "Viewer")
        self.assertEqual(result["name"], "Trips")

    def test_create_writes_uuid_direct_dataset_and_config_envelope(self):
        created = {"id": "chart-uuid"}
        with patch.object(charts.uuid, "uuid4", return_value="chart-uuid"), \
             patch.object(charts.db, "execute") as execute, \
             patch.object(charts, "get_chart_by_id", return_value=created):
            result = charts.create_chart({
                "name": "Trips", "dataset_id": 7, "chart_type": "bar",
                "query_config": {"groupby": ["borough"]}, "viz_config": {"legend": True},
            }, "seed@example.com")
        self.assertEqual(result, created)
        statement, params = execute.call_args.args
        self.assertIn("(id, name, description, dataset_id, chart_type, config", statement)
        self.assertEqual(params[:5], ["chart-uuid", "Trips", None, 7, "bar"])
        self.assertEqual(json.loads(params[5]), {
            "query_config": {"dataset_id": 7, "groupby": ["borough"]}, "viz_config": {"legend": True},
        })

    def test_update_preserves_query_config_when_changing_visual_config(self):
        existing = {"id": "chart-uuid", "query_config": {"dataset_id": 7}, "viz_config": {"legend": False}}
        with patch.object(charts, "get_chart_by_id", side_effect=[existing, existing, existing]), \
             patch.object(charts.db, "execute") as execute:
            charts.update_chart("chart-uuid", {"viz_config": {"legend": True}})
        params = execute.call_args.args[1]
        saved_config = next(json.loads(value) for value in params if isinstance(value, str) and value.startswith("{"))
        self.assertEqual(saved_config, {"query_config": {"dataset_id": 7}, "viz_config": {"legend": True}})
        self.assertEqual(params[-1], "chart-uuid")

    def test_capability_probe_recognizes_only_known_modern_or_legacy_layouts(self):
        self.schema.stop()
        charts._schema_cache = None
        modern = [{"column_name": name} for name in ("id", "dataset_id", "config", "modified_at", "modified_by")]
        with patch.object(charts.db, "query", return_value={"rows": modern}):
            self.assertEqual(charts._chart_schema(), "modern")
        charts._schema_cache = None
        legacy = [{"column_name": name} for name in ("id", "query_config", "viz_config", "updated_at", "updated_by")]
        with patch.object(charts.db, "query", return_value={"rows": legacy}):
            self.assertEqual(charts._chart_schema(), "legacy")
        self.schema.start()

    def test_legacy_crud_uses_legacy_config_columns_and_integer_id(self):
        self.schema.stop()
        with patch.object(charts, "_chart_schema", return_value="legacy"), \
             patch.object(charts.db, "execute") as execute, \
             patch.object(charts.db, "query_one", side_effect=[{"id": 9}, {"id": 9, "query_config": "{}", "viz_config": "{}"}]):
            created = charts.create_chart({"name": "Legacy", "dataset_id": 7, "chart_type": "bar"}, "admin@example.com")
        statement = execute.call_args.args[0]
        self.assertIn("query_config, viz_config", statement)
        self.assertEqual(created["id"], "9")
        self.schema.start()

    def test_legacy_update_and_delete_keep_integer_identifier(self):
        self.schema.stop()
        row = {"id": 9, "query_config": "{}", "viz_config": "{}"}
        with patch.object(charts, "_chart_schema", return_value="legacy"), \
             patch.object(charts.db, "query_one", side_effect=[row, row]), \
             patch.object(charts.db, "execute", return_value=1) as execute:
            charts.update_chart("9", {"query_config": {"dataset_id": 7}})
            self.assertTrue(charts.delete_chart("9"))
        update_params = execute.call_args_list[0].args[1]
        self.assertEqual(update_params[-1], 9)
        self.assertEqual(execute.call_args_list[1].args[1], [9])
        self.schema.start()


if __name__ == "__main__":
    unittest.main()
