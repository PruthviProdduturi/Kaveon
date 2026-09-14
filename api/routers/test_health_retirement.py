import unittest
from unittest.mock import patch

from fastapi.responses import JSONResponse

from routers import health


class RetirementHealthTests(unittest.TestCase):
    def test_health_reports_kaveondb_as_authority_without_postgresql_probe(self):
        state = {"authority_family_count": 16}
        with patch.object(health.postgresql_retirement_runtime, "requested", return_value=True), \
             patch.object(health.postgresql_retirement_runtime, "probe", return_value=state), \
             patch.object(health.db, "query") as postgres:
            result = health.health()
        self.assertEqual((result["status"], result["authority"]), ("healthy", "kaveondb"))
        self.assertFalse(result["checks"]["postgresql"]["required"])
        postgres.assert_not_called()

    def test_health_fails_closed_without_exposing_validation_details(self):
        with patch.object(health.postgresql_retirement_runtime, "requested", return_value=True), \
             patch.object(health.postgresql_retirement_runtime, "probe", side_effect=RuntimeError("secret path")):
            result = health.health()
        self.assertIsInstance(result, JSONResponse)
        self.assertEqual(result.status_code, 503)
        self.assertNotIn(b"secret path", result.body)


if __name__ == "__main__":
    unittest.main()
