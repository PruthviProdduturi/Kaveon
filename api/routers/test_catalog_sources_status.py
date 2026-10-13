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

from fastapi import HTTPException

from middleware.auth import UserContext
from routers.catalog_sources import engine_status


ADMIN = UserContext(email="admin@example.test", role="Admin")


class EngineStatusTests(unittest.TestCase):
    """Whether the Engine is configured and whether it answered are two
    questions, and this route used to collapse them into one.

    It raised 502 when the coordinator did not answer, so Studio received no
    response at all and told the administrator to set KAVEON_ENGINE_URL and the
    bridge credential — the one thing that was demonstrably already right. A
    coordinator restarting during a rollout is not a configuration error.
    """

    @patch("services.engine_bridge.configured", return_value=True)
    @patch("services.engine_bridge.catalogs")
    def test_status_reports_connectivity_without_endpoint_or_credentials(self, catalogs, _configured):
        catalogs.return_value = {"catalogs": ["medallion", "analytics"]}

        result = engine_status(ADMIN)

        self.assertEqual(
            result,
            {"success": True, "configured": True, "connected": True, "catalog_count": 2},
        )
        catalogs.assert_called_once_with("admin@example.test", "Admin")
        self.assertNotIn("endpoint", result)
        self.assertNotIn("url", result)
        self.assertNotIn("token", result)

    @patch("services.engine_bridge.configured", return_value=False)
    @patch("services.engine_bridge.catalogs")
    def test_an_unconfigured_deployment_is_not_asked(self, catalogs, _configured):
        result = engine_status(ADMIN)
        self.assertEqual(result, {"success": True, "configured": False, "connected": False,
                                  "catalog_count": None, "reason": "not_configured"})
        # Nothing to reach, so nothing is reached.
        catalogs.assert_not_called()

    @patch("services.engine_bridge.configured", return_value=True)
    @patch("services.engine_bridge.catalogs",
           side_effect=HTTPException(502, "Engine is unavailable"))
    def test_a_coordinator_that_does_not_answer_is_reachability_not_configuration(
            self, _catalogs, _configured):
        result = engine_status(ADMIN)
        self.assertEqual(result, {"success": True, "configured": True, "connected": False,
                                  "catalog_count": None, "reason": "unreachable",
                                  "upstream_status": 502})

    @patch("services.engine_bridge.configured", return_value=True)
    @patch("services.engine_bridge.catalogs",
           side_effect=HTTPException(504, "Engine statement exceeded the client bound"))
    def test_a_timed_out_coordinator_reads_the_same_way(self, _catalogs, _configured):
        self.assertEqual(engine_status(ADMIN)["reason"], "unreachable")

    @patch("services.engine_bridge.configured", return_value=True)
    @patch("services.engine_bridge.catalogs", return_value={"catalogs": "not a list"})
    def test_an_answer_that_is_not_a_listing_is_reported_as_disagreement(
            self, _catalogs, _configured):
        result = engine_status(ADMIN)
        # Reached and answered, just not with a catalog listing. Not an absent
        # deployment setting, so not reported as one.
        self.assertEqual(result["configured"], True)
        self.assertEqual(result["connected"], False)
        self.assertEqual(result["reason"], "invalid_listing")

    @patch("services.engine_bridge.configured", return_value=True)
    @patch("services.engine_bridge.catalogs",
           side_effect=HTTPException(403, "Engine rejected the credential"))
    def test_a_refused_credential_still_surfaces_as_an_error(self, _catalogs, _configured):
        # Only reachability is translated. Anything else is the caller's to see.
        with self.assertRaises(HTTPException) as raised:
            engine_status(ADMIN)
        self.assertEqual(raised.exception.status_code, 403)
