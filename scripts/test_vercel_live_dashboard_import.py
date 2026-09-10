import copy
import importlib.util
import json
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location("live_import", Path(__file__).with_name("import-vercel-live-dashboards.py"))
module = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(module)


class LiveDashboardImportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.contract = json.loads((Path(__file__).parents[1] / "data/dashboard-templates/vercel-live-dashboard-contract.json").read_text())

    def test_canonical_contract_is_exact_eight_and_seventy(self):
        module.validate_contract(self.contract)
        self.assertEqual(len(self.contract["dashboards"]), 8)
        self.assertEqual(len(self.contract["charts"]), 70)

    def test_chart_remap_preserves_configs_and_marks_source(self):
        chart = self.contract["charts"][0]
        ids = {legacy: index + 1 for index, legacy in enumerate(module.PHYSICAL_DATASETS)}
        result = module.chart_body(chart, ids)
        self.assertEqual(result["viz_config"], chart["viz_config"])
        self.assertEqual(result["query_config"][module.MARKER], f"vercel-chart:{chart['id']}")
        self.assertNotIn("datasource", result["query_config"])
        self.assertEqual(result["dataset_id"], ids[str(chart["dataset_id"])])

    def test_dashboard_remaps_layout_charts_and_filter_datasets(self):
        dashboard = next(row for row in self.contract["dashboards"] if row["filters"] != "[]")
        chart_ids = {str(chart["id"]): f"new-{chart['id']}" for chart in self.contract["charts"]}
        dataset_ids = {legacy: index + 1000 for index, legacy in enumerate(module.PHYSICAL_DATASETS)}
        result = module.dashboard_body(dashboard, chart_ids, dataset_ids)
        visual_layout = result["layout"]
        original_layout = json.loads(dashboard["layout"])
        self.assertEqual(len(visual_layout), len(original_layout))
        self.assertEqual(result["charts"], [chart_ids[str(value)] for value in json.loads(dashboard["charts"])])
        self.assertTrue(all(row["datasetId"] in dataset_ids.values() for row in result["filters"]))
        self.assertEqual(module.object_marker(result, "dashboard"), f"vercel-dashboard:{dashboard['id']}")

    def test_rejects_incomplete_contract_before_writes(self):
        broken = copy.deepcopy(self.contract)
        broken["charts"].pop()
        with self.assertRaisesRegex(RuntimeError, "exact 8/70"):
            module.validate_contract(broken)

    def test_all_33_event_charts_fit_lossless_projection(self):
        event_charts = [chart for chart in self.contract["charts"] if str(chart["dataset_id"]) == "144"]
        self.assertEqual(len(event_charts), 33)
        module.validate_event_projection_contract(self.contract)
        self.assertEqual(module.PHYSICAL_DATASETS["144"]["table_name"], "kaveon_events_dashboard")

    def test_rejects_event_query_outside_lossless_projection(self):
        broken = copy.deepcopy(self.contract)
        chart = next(chart for chart in broken["charts"] if str(chart["dataset_id"]) == "144")
        chart["query_config"]["groupby"] = ["event_date"]
        with self.assertRaisesRegex(RuntimeError, "exceeds compact projection"):
            module.validate_event_projection_contract(broken)

    def test_dataset_semantics_cover_exact_chart_and_filter_contract(self):
        event = module.dataset_semantics(self.contract, "144")
        self.assertEqual(event["dimensions"], module.EVENT_DIMENSIONS)
        self.assertIn("user_id", event["metric_columns"])
        expressions = {metric["expression"] for metric in event["metrics"]}
        self.assertIn("COUNT(DISTINCT user_id)", expressions)
        self.assertIn("AVG(latency_p75_ms)", expressions)
        self.assertEqual(len(expressions), len(event["metrics"]))

        energy = module.dataset_semantics(self.contract, "132")
        self.assertTrue({"country", "iso_code", "year"}.issubset(energy["dimensions"]))
        self.assertIn("SUM(primary_energy_consumption)", {m["expression"] for m in energy["metrics"]})

    def test_cleanup_only_selects_stale_namespaced_marker(self):
        wanted = {"layout": [{module.MARKER: "vercel-dashboard:wanted"}]}
        stale = {"layout": [{module.MARKER: "vercel-dashboard:stale"}]}
        unrelated = {"layout": [{module.MARKER: "another-importer:stale"}]}
        expected = {"vercel-dashboard:wanted"}
        candidates = [item for item in (wanted, stale, unrelated) if (module.object_marker(item, "dashboard") or "").startswith("vercel-dashboard:") and module.object_marker(item, "dashboard") not in expected]
        self.assertEqual(candidates, [stale])

    def test_read_only_audit_reports_all_tables_and_stale_cleanup_without_deleting(self):
        calls = []
        datasets = [{"id": 42, "database_name": "OpenSource", "schema_name": "climate_energy", "table_name": "energy_annual"}]
        charts = [{"query_config": {module.MARKER: f"vercel-chart:{self.contract['charts'][0]['id']}"}}]
        dashboards = [
            {"layout": [{module.MARKER: f"vercel-dashboard:{self.contract['dashboards'][0]['id']}"}]},
            {"layout": [{module.MARKER: "vercel-dashboard:stale"}]},
            {"layout": [{module.MARKER: "unrelated:keep"}]},
        ]
        def api(method, path, body=None):
            calls.append((method, path))
            if path == "lab/engine/sources": return {"sources": [{"id": "open", "catalog": "OpenSource"}]}
            if path == "datasets": return datasets
            if path == "charts": return charts
            if path == "dashboards": return dashboards
            if path.endswith("/columns"): return {"schema": {"columns": [{"name": "x", "dataType": "Int64"}]}}
            raise AssertionError(path)
        result = module.audit_contract(self.contract, api)
        self.assertTrue(result["ready_to_apply"])
        self.assertEqual(len(result["physical_datasets"]), 9)
        self.assertEqual(result["registered_dataset_count"], 1)
        self.assertEqual(result["cleanup_would_delete_count"], 1)
        self.assertFalse(any(method != "GET" for method, _ in calls))


if __name__ == "__main__":
    unittest.main()
