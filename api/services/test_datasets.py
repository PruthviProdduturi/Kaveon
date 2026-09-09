import json
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import datasets


class DatasetUpdateTests(unittest.TestCase):
    def test_virtual_sql_and_unrelated_metadata_survive_filter_refresh(self):
        existing = {
            "id": "7",
            "tables_used": json.dumps({
                "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard",
                "filters": [{"column": "model"}],
                "seed_revision": "showcase-v1",
            }),
        }
        with patch.object(datasets, "get_dataset_by_id", side_effect=[existing, {"id": "7"}]), \
             patch.object(datasets.db, "execute") as execute:
            result = datasets.update_dataset(
                "7",
                {"sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard WHERE score IS NOT NULL",
                 "filters": [{"column": "provider"}]},
                "seed@example.com",
            )
        self.assertEqual(result, {"id": "7"})
        updated_metadata = next(
            value for value in execute.call_args.args[1] if isinstance(value, str) and "sql_text" in value
        )
        self.assertEqual(json.loads(updated_metadata), {
            "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard WHERE score IS NOT NULL",
            "filters": [{"column": "provider"}],
            "seed_revision": "showcase-v1",
        })


if __name__ == "__main__":
    unittest.main()
