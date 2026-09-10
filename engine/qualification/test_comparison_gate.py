from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from comparison_gate import evaluate


def analytics(ratio=1.9):
    return {"same_file_correctness": True, "fair_performance_comparison": True,
            "publication_workload_gate": True,
            "cases": [{"passed": True, "result_sha256": "a" * 64}],
            "throughput": {"passed": True, "kaveon_over_trino": ratio}}


def transactions(qps=1.01, p95=.99):
    names = ["point_read", "insert", "update", "delete", "conflicting_update", "multi_record_commit"]
    return {"resources_matched": True, "publication_workload_gate": True,
            "correctness_passed": True, "kaveon_over_postgresql_qps": qps,
            "kaveon_over_postgresql_p95_ratio": p95,
            "operations": [{"name": name, "samples": 30, "passed": True,
                            "state_sha256": "b" * 64} for name in names]}


class ComparisonGateTests(unittest.TestCase):
    def test_missing_evidence_is_pending_and_never_qualified(self):
        result = evaluate(None, None)
        self.assertFalse(result["qualified"])
        self.assertEqual(result["analytics_vs_trino"]["status"], "pending")
        self.assertEqual(result["transactions_vs_postgresql"]["status"], "pending")

    def test_both_independent_suites_must_pass(self):
        self.assertTrue(evaluate(analytics(), transactions())["qualified"])
        self.assertFalse(evaluate(analytics(1.89), transactions())["qualified"])
        self.assertFalse(evaluate(analytics(), transactions(p95=1.0))["qualified"])

    def test_correctness_hash_and_full_transaction_corpus_are_mandatory(self):
        analytical = analytics()
        analytical["cases"][0].pop("result_sha256")
        self.assertFalse(evaluate(analytical, transactions())["qualified"])
        transactional = transactions()
        transactional["operations"].pop()
        result = evaluate(analytics(), transactional)
        self.assertFalse(result["qualified"])
        self.assertIn("multi_record_commit", result["transactions_vs_postgresql"]["reasons"][-1])


if __name__ == "__main__":
    unittest.main()
