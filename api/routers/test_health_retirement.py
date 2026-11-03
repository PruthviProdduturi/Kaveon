import unittest
from unittest.mock import patch

from fastapi.responses import JSONResponse

from routers import health


class RetirementHealthTests(unittest.TestCase):
    def test_live_does_not_probe_authority(self):
        """Liveness is about the process. It must not wait on the Engine."""
        with patch.object(health.postgresql_retirement_runtime, "probe") as probe:
            self.assertEqual(health.live(), {"status": "alive"})
        probe.assert_not_called()

    def test_health_reports_kaveondb_as_the_only_authority(self):
        """There is no second system to report on. The `postgresql` key that
        used to say `required: false`, and the `data_warehouse` beside it, are
        gone with the branch that produced them: naming a database the product
        does not have, only to say it is not needed, described an absence
        rather than a state."""
        state = {"authority_family_count": 16}
        with patch.object(health.postgresql_retirement_runtime, "probe", return_value=state):
            result = health.health()
        self.assertEqual((result["status"], result["authority"]), ("healthy", "kaveondb"))
        self.assertEqual(sorted(result["checks"]), ["azure_ad", "kaveondb"])
        self.assertTrue(result["checks"]["kaveondb"]["authoritative"])
        self.assertEqual(result["checks"]["kaveondb"]["authority_family_count"], 16)
        self.assertFalse(hasattr(health, "db"), "no metadata database to probe")

    def test_health_fails_closed_without_exposing_validation_details(self):
        with patch.object(health.postgresql_retirement_runtime, "probe",
                          side_effect=RuntimeError("secret path")):
            result = health.health()
        self.assertIsInstance(result, JSONResponse)
        self.assertEqual(result.status_code, 503)
        self.assertNotIn(b"secret path", result.body)


if __name__ == "__main__":
    unittest.main()
