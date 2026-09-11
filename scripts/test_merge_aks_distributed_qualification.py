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
            "checks": {"worker_loss_exact_retry_and_recovery": True},
            "concurrent_exact": {"requests": [{"exact": True, "status": 200}]},
            "worker_loss": {"exact": True, "retry": {"attempt_incremented": True}},
            "pressure": {"sampled_engine_memory_within_pod_limits": True},
            "retained_files_after": {
                "coordinator_exchange": 0 if cleanup else 2,
                "worker-0_spill": 0,
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


if __name__ == "__main__":
    unittest.main()
