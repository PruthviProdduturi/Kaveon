import importlib.util
import unittest
from pathlib import Path


MODULE = Path(__file__).with_name("merge-aks-distributed-qualification.py")
SPEC = importlib.util.spec_from_file_location("merge_aks", MODULE)
merge_aks = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(merge_aks)


class MergeQualificationTests(unittest.TestCase):
    def comparison(self):
        return {
            "passed": True,
            "preflight": {"kaveon_image_pinned_and_expected": True},
            "cases": [{
                "kaveon_ms": [10, 20, 30, 40],
                "kaveon_execution_by_stage": {"0": {
                    "compute_cpu_us": 10,
                    "compute_wall_us": 20,
                    "exchange_input_bytes": 100,
                    "exchange_output_bytes": 200,
                    "exchange_output_copies": 2,
                    "exchange_hash_us": 3,
                    "exchange_copy_us": 4,
                    "exchange_copy_allocations": 5,
                    "exchange_copied_bytes": 600,
                    "spill_bytes_written": 700,
                    "spill_runs_written": 2,
                    "spill_compactions": 1,
                    "memory_peak_bytes": 800,
                    "spill_peak_bytes": 900,
                }},
            }],
            "throughput": {"passed": True, "aggregate_qps": {"kaveon": 2.0}},
        }

    def pressure(self, cleanup=True):
        return {
            "checks": {"worker_loss_exact_retry_and_recovery": True, "cancellation_cleanup": True},
            "concurrent_exact": {"requests": [{"exact": True, "status": 200}]},
            "worker_loss": {"exact": True, "retry": {"attempt_incremented": True}},
            "pressure": {"sampled_engine_memory_within_pod_limits": True},
            "retained_files_after": {
                "coordinator_exchange": 0 if cleanup else 2,
                "worker-0_spill": 0,
            },
        }

    def current_pressure(self, cleanup=True):
        """Shape emitted by the live pod-loss runner."""
        return {
            "checks": {
                "worker_loss_exact_retry_and_recovery": True,
                "cancellation_cleanup": True,
            },
            "pressure": {
                "requests": [
                    {"exact": True, "status": 200, "admission_refusals": 2},
                    {"exact": True, "status": 200, "admission_refusals": 0},
                    {"exact": False, "status": 429},
                ],
                "sampled_engine_memory_within_pod_limits": True,
            },
            "worker_loss": {
                "exact_match": True,
                "retry_attempt_observed": True,
            },
            "retained_files_after": {
                "coordinator_exchange": 0 if cleanup else 2,
                "kaveon-worker-0_spill": 0,
            },
        }

    def test_passes_only_when_all_recovery_gates_pass(self):
        result = merge_aks.merge(self.comparison(), self.pressure())
        self.assertTrue(result["passed"])
        self.assertEqual(result["latency"]["p50_ms"], 20)
        self.assertEqual(result["latency"]["p95_ms"], 40)
        self.assertEqual(result["latency"]["p99_ms"], 40)
        self.assertEqual(result["execution_metrics"]["totals"]["spill_bytes_written"], 700)
        self.assertEqual(result["execution_metrics"]["peaks"]["memory_peak_bytes"], 800)

    def test_coordinator_retained_chunks_keep_report_pending(self):
        result = merge_aks.merge(self.comparison(), self.pressure(cleanup=False))
        self.assertFalse(result["passed"])
        self.assertFalse(result["gates"]["coordinator_restart_cleanup"])
        self.assertFalse(result["slo"]["passed"])

    def test_slo_fails_closed_when_cancellation_evidence_is_missing(self):
        pressure = self.pressure()
        pressure["checks"].pop("cancellation_cleanup")
        result = merge_aks.merge(self.comparison(), pressure)
        self.assertFalse(result["slo"]["passed"])
        self.assertFalse(result["slo"]["requirements"]["cancellation_cleanup"])

    def test_slo_rejects_an_accepted_query_success_rate_below_999_percent(self):
        pressure = self.pressure()
        pressure["concurrent_exact"]["requests"] = [
            {"exact": True, "status": 200}
        ] * 998 + [{"exact": False, "status": 500}] * 2
        result = merge_aks.merge(self.comparison(), pressure)
        self.assertAlmostEqual(result["slo"]["accepted_success_rate"], 0.998, places=6)
        self.assertFalse(result["slo"]["requirements"]["accepted_success_rate"])
        self.assertFalse(result["slo"]["passed"])

    def test_slo_excludes_admission_refusals_from_accepted_rate(self):
        pressure = self.pressure()
        pressure["concurrent_exact"]["requests"] = [
            {"exact": True, "status": 200},
            {"exact": True, "status": 200},
            {"exact": False, "status": 429},
        ]
        result = merge_aks.merge(self.comparison(), pressure)
        self.assertEqual(result["slo"]["accepted_requests"], 2)
        self.assertEqual(result["slo"]["admission_refusals"], 1)
        self.assertEqual(result["slo"]["accepted_success_rate"], 1.0)
        self.assertTrue(result["slo"]["requirements"]["accepted_success_rate"])

    def test_merges_current_live_fault_pressure_shape(self):
        result = merge_aks.merge(self.comparison(), self.current_pressure())
        self.assertTrue(result["gates"]["worker_retry"])
        self.assertEqual(result["slo"]["accepted_requests"], 2)
        self.assertEqual(result["slo"]["admission_refusals"], 3)
        self.assertTrue(result["slo"]["requirements"]["cancellation_cleanup"])

    def test_reads_nested_preflight_image_binding_from_archived_comparison(self):
        comparison = self.comparison()
        comparison["preflight"] = {
            "checks": {"kaveon_image_pinned_and_expected": True}
        }
        result = merge_aks.merge(comparison, self.pressure())
        self.assertTrue(result["slo"]["requirements"]["image_binding"])

    def test_live_fault_report_must_match_comparison_engine_digest(self):
        comparison = self.comparison()
        comparison["preflight"] = {
            "checks": {"kaveon_image_pinned_and_expected": True},
            "images": {
                "kaveon_coordinator": "registry/kaveon-engine@sha256:old",
                "kaveon_worker": "registry/kaveon-engine@sha256:old",
            },
        }
        pressure = self.pressure()
        pressure["pods_before"] = {
            "kaveon-coordinator-0": {
                "image": "registry/kaveon-engine@sha256:new"
            }
        }
        result = merge_aks.merge(comparison, pressure)
        self.assertFalse(result["slo"]["requirements"]["image_binding"])
        self.assertFalse(result["slo"]["passed"])

    def test_current_live_shape_stays_pending_when_cleanup_is_missing(self):
        result = merge_aks.merge(self.comparison(), self.current_pressure(cleanup=False))
        self.assertFalse(result["passed"])
        self.assertFalse(result["gates"]["coordinator_restart_cleanup"])


if __name__ == "__main__":
    unittest.main()
