import csv
import importlib.util
import tempfile
import unittest
from pathlib import Path

import pyarrow.parquet as pq


SPEC = importlib.util.spec_from_file_location("climate", Path(__file__).with_name("curate-original-climate-dashboards.py"))
climate = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(climate)


class OriginalClimateDashboardTests(unittest.TestCase):
    def test_restores_exact_historical_source_blobs(self):
        with tempfile.TemporaryDirectory() as directory:
            temperature, energy = climate.restore_pinned_sources(Path(directory))
            self.assertEqual(temperature.stat().st_size, 574626)
            self.assertEqual(energy.stat().st_size, 172607)

    def test_materializes_canonical_cross_domain_aliases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (root / "temperature.csv").open("w", newline="", encoding="utf-8") as stream:
                writer = csv.DictWriter(stream, fieldnames=["country", "country_code", "year", "month", "temp_change_c"])
                writer.writeheader(); writer.writerows([
                    {"country": "A", "country_code": "AAA", "year": 2023, "month": 1, "temp_change_c": 1.2},
                    {"country": "A", "country_code": "AAA", "year": 2023, "month": 2, "temp_change_c": 1.8},
                ])
            with (root / "energy.csv").open("w", newline="", encoding="utf-8") as stream:
                writer = csv.DictWriter(stream, fieldnames=climate.ENERGY_COLUMNS); writer.writeheader()
                row = {name: "" for name in climate.ENERGY_COLUMNS}
                row.update({"country": "A", "iso_code": "AAA", "year": "2023", "population": "10", "primary_energy_consumption": "20"})
                writer.writerow(row)
            manifest = climate.materialize(root / "temperature.csv", root / "energy.csv", root / "out")
            cross = pq.read_table(root / "out/climate_energy/climate_x_energy/part-00000.parquet")
            self.assertIn("avg_tc", cross.column_names)
            self.assertNotIn("avg_temp_change_c", cross.column_names)
            self.assertEqual(cross["avg_tc"].to_pylist(), [1.5])
            self.assertEqual([table["name"] for table in manifest["tables"]], ["temperature_monthly", "energy_annual", "climate_x_energy"])


if __name__ == "__main__":
    unittest.main()
