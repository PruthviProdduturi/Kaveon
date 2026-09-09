from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest

import pyarrow as pa
import pyarrow.parquet as pq


SCRIPT = Path(__file__).with_name("project-kaveon-events-dashboard.py")
SPEC = importlib.util.spec_from_file_location("project_kaveon_events_dashboard", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


class ProjectionTest(unittest.TestCase):
    def test_materializes_complete_deterministic_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = []
            for user_id in range(1, 13):
                rows.append({
                    "usage_date": "2026-09-09", "user_id": user_id,
                    "queries_run": 20, "nl_queries": 2, "sql_lab_runs": 3,
                    "dashboards_viewed": 4, "charts_created": 5, "exports": 1,
                    "api_calls": 6, "data_processed_mb": 1.5, "errors": 2,
                    "sessions": 3, "license": "Pro", "segment": "Enterprise",
                    "industry": "Software", "team_size": "51-200",
                    "deployment": "AKS", "acquisition_channel": "Organic",
                    "country": "US", "region": "North America", "platform": "Web",
                })
            source = root / "source.parquet"
            pq.write_table(pa.Table.from_pylist(rows), source)
            manifest = MODULE.materialize(source, root / "out", batch_rows=3)
            table = pq.read_table(root / "out/public/kaveon_events_dashboard/part-00000.parquet")
            self.assertEqual(table.num_rows, 12)
            self.assertEqual(table.schema.names, MODULE.OUTPUT_SCHEMA.names)
            self.assertEqual(set(table["surface"].to_pylist()), set(MODULE.SURFACES))
            first = table.to_pylist()[0]
            self.assertEqual(first["actions"], 21)
            self.assertEqual(first["rows_scanned"], 1536)
            self.assertLessEqual(first["cache_hits"], first["queries_run"])
            self.assertFalse(manifest["tables"][0]["lineage"]["legacy_equivalent"])
            self.assertEqual(
                MODULE.project(rows[0]), MODULE.project(rows[0]),
                "the projection must be deterministic",
            )


if __name__ == "__main__":
    unittest.main()
