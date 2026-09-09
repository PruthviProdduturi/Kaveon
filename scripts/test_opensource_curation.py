"""Small fixtures for the data rules; does not download or contact Azure."""
import importlib.util
import tempfile
import unittest
from collections import defaultdict
from datetime import datetime
from pathlib import Path
import pyarrow as pa
import pyarrow.parquet as pq

spec = importlib.util.spec_from_file_location('nyc', Path(__file__).with_name('curate-nyc-taxi.py'))
nyc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nyc)

class NycCurationTests(unittest.TestCase):
    def test_raw_clean_rejected_and_gold_reconcile(self):
        table = pa.table({
            'tpep_pickup_datetime': [datetime(2025,1,2), datetime(2024,12,31), datetime(2025,1,3)],
            'tpep_dropoff_datetime': [datetime(2025,1,2,1), datetime(2025,1,1), datetime(2025,1,3,1)],
            'trip_distance': [1.0, 2.0, float('nan')],
            'total_amount': [10.125, 5.0, 6.0],
        })
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            pq.write_table(table, work/'source.parquet')
            aggregate = defaultdict(lambda: {'trip_count':0,'total_amount_cents':0,'total_trip_distance':0.0})
            tables, stats = nyc.curate_trips('yellow', work/'source.parquet', work, aggregate)
            self.assertEqual(stats, {'source_count':3,'accepted':1,'rejected':2})
            self.assertEqual([pq.ParquetFile(work/t['location']).metadata.num_rows for t in tables], [3,1,2])
            rejected = pq.read_table(work/tables[2]['location'])['rejection_reason'].to_pylist()
            self.assertEqual(rejected, ['pickup_outside_january_2025','trip_distance_not_finite_nonnegative'])
            _, gold = nyc.write_gold(work, aggregate)
            self.assertEqual(gold['gold_trip_count'], 1)
            self.assertEqual(gold['gold_total_amount_cents'], 1013)

if __name__ == '__main__':
    unittest.main()
