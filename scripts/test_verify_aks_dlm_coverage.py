import importlib.util
from pathlib import Path
import unittest


SCRIPT = Path(__file__).with_name("verify-aks-dlm-coverage.py")
SPEC = importlib.util.spec_from_file_location("verify_aks_dlm_coverage", SCRIPT)
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class CoverageValidationTests(unittest.TestCase):
    def test_accepts_all_expected_ready_positive_counts(self):
        payload = {"datasets": [
            {"dataset_id": index, "name": name, "status": "ready", "row_count": index + 1,
             "row_count_source": "kaveon_engine_exact"}
            for index, name in enumerate(verify.EXPECTED_DATASETS)
        ]}
        checks, errors = verify.validate_coverage(payload)
        self.assertEqual(errors, [])
        self.assertEqual(len(checks), 9)
        self.assertTrue(all(check["passed"] for check in checks))

    def test_rejects_missing_nonready_and_unknown_counts(self):
        payload = {"datasets": [
            {"name": verify.EXPECTED_DATASETS[0], "status": "building", "row_count": None},
        ]}
        checks, errors = verify.validate_coverage(payload)
        self.assertFalse(checks[0]["passed"])
        self.assertEqual(len(errors), 11)
        self.assertTrue(any("expected ready" in error for error in errors))
        self.assertTrue(any("positive exact row_count" in error for error in errors))
        self.assertTrue(any("kaveon_engine_exact" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
