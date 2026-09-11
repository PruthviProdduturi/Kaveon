import sys
import unittest

from trino_benchmark_preflight import command_result, classify_docker_failure, required_checks_pass


class TrinoBenchmarkPreflightTests(unittest.TestCase):
    def test_classifies_managed_defender_wsl_failure(self):
        result = classify_docker_failure(
            "A fatal error was returned by plugin 'DefenderforEndpointPlug-in'.",
            "Wsl/Service/CreateInstance/CreateVm/Plugin/E_ABORT",
        )
        self.assertEqual(result["code"], "windows_wsl_defender_plugin_failure")
        self.assertIn("do not weaken", result["remediation"])

    def test_classifies_unreachable_docker_engine(self):
        result = classify_docker_failure("failed to connect to the docker daemon")
        self.assertEqual(result["code"], "docker_engine_unavailable")

    def test_readiness_fails_closed(self):
        self.assertTrue(required_checks_pass({"docker": True, "image": True}))
        self.assertFalse(required_checks_pass({"docker": True, "image": False}))
        self.assertFalse(required_checks_pass({"docker": True, "image": None}))
        self.assertFalse(required_checks_pass({}))

    def test_command_can_preserve_structured_output_for_inspection(self):
        result = command_result([sys.executable, "-c", "print('x' * 5000)"], output_limit=None)
        self.assertEqual(result["returncode"], 0)
        self.assertEqual(len(result["stdout"]), 5000)


if __name__ == "__main__":
    unittest.main()
