import os
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from fastapi import HTTPException
from database import product_engine

try:
    import pyodbc  # noqa: F401
except ModuleNotFoundError:
    sys.modules["pyodbc"] = SimpleNamespace(Connection=object, connect=lambda *args, **kwargs: None)

from database import metadata


class ProductEngineClientTests(unittest.TestCase):
    def setUp(self):
        self.environment = patch.dict(os.environ, {"KAVEON_ENGINE_CATALOG_TOKEN": "secret"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    @patch("database.product_engine._verify_context", return_value=True)
    @patch("database.product_engine._endpoint", return_value="https://engine.test")
    @patch("database.product_engine.httpx.post")
    def test_one_atomic_request_preserves_prior_result_binding(self, post, endpoint, verify):
        response = Mock()
        response.is_success = True
        response.json.return_value = {
            "results": [
                {"rows": [{"id": 7}], "row_count": 1},
                {"rows": [], "row_count": 1},
            ]
        }
        post.return_value = response

        result = product_engine.transaction([
            {"sql": "INSERT INTO datasets(name) VALUES (?) RETURNING id", "params": ["orders"]},
            {"sql": "INSERT INTO dataset_columns(dataset_id) VALUES (?)", "params": [{"$result": {"statement": 0, "column": "id"}}]},
        ])

        self.assertEqual(result[0]["rows"][0]["id"], 7)
        self.assertEqual(post.call_count, 1)
        request = post.call_args.kwargs
        self.assertEqual(request["json"]["statements"][1]["params"][0], {"$result": {"statement": 0, "column": "id"}})
        self.assertFalse(request["json"]["read_only"])
        self.assertEqual(request["headers"], {"Authorization": "Bearer secret"})

    def test_rejects_forward_binding_before_network_request(self):
        with patch("database.product_engine.httpx.post") as post:
            with self.assertRaises(ValueError):
                product_engine.transaction([
                    {"sql": "SELECT 1", "params": [{"$result": {"statement": 0, "column": "id"}}]},
                ])
        post.assert_not_called()

    def test_missing_credential_is_not_sent_or_logged(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(HTTPException) as error:
                product_engine.transaction([{"sql": "SELECT 1", "params": []}])
        self.assertEqual(error.exception.status_code, 503)

    def test_read_only_is_conservative(self):
        self.assertTrue(product_engine._read_only("SELECT 1"))
        self.assertFalse(product_engine._read_only("WITH x AS (SELECT 1) SELECT * FROM x"))
        self.assertFalse(product_engine._read_only("INSERT INTO x VALUES (1)"))

    @patch("database.product_engine.transaction", return_value=[{"rows": [], "row_count": 1}])
    def test_metadata_routes_engine_backend_and_adapts_sqlite_syntax(self, transaction):
        with patch.dict(os.environ, {"METADATA_DB_TYPE": "kaveon_engine"}):
            result = metadata.query("SELECT TOP 1 GETUTCDATE() AS now FROM dbo.datasets", [])
        self.assertEqual(result["row_count"], 1)
        statement = transaction.call_args.args[0][0]
        self.assertEqual(statement["sql"], "SELECT CURRENT_TIMESTAMP AS now FROM datasets LIMIT 1")
