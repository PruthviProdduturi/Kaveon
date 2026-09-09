import unittest
import sys
from types import SimpleNamespace
from unittest.mock import patch

# Router import normally reaches the database pool. This focused route test does
# not open a database connection, and the local Python test image omits pyodbc.
try:
    import pyodbc  # noqa: F401
except ModuleNotFoundError:
    sys.modules["pyodbc"] = SimpleNamespace(Connection=object, connect=lambda *args, **kwargs: None)

from middleware.auth import UserContext
from routers.catalog_sources import engine_status


class EngineStatusTests(unittest.TestCase):
    @patch("services.engine_bridge.catalogs")
    def test_status_reports_connectivity_without_endpoint_or_credentials(self, catalogs):
        catalogs.return_value = {"catalogs": ["medallion", "analytics"]}

        result = engine_status(UserContext(email="admin@example.test", role="Admin"))

        self.assertEqual(
            result,
            {"success": True, "configured": True, "connected": True, "catalog_count": 2},
        )
        catalogs.assert_called_once_with("admin@example.test", "Admin")
        self.assertNotIn("endpoint", result)
        self.assertNotIn("url", result)
        self.assertNotIn("token", result)
