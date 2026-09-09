import json
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import charts


class ChartPostgresSchemaTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
