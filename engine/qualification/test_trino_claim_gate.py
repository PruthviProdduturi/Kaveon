import copy
import hashlib
import json
import unittest

from same_files import EXTENDED_QUERIES
from trino_claim_gate import evaluate


def valid_report():
    cases = [{"name": name, "sql": sql, "passed": True, "result_sha256": "a" * 64,
              "kaveon_ms": [1] * 30, "trino_ms": [2] * 30,
              "median_ms": {"kaveon": 1, "trino": 2},
              "statistics": {engine: {"min_ms": 1, "p95_ms": 2, "max_ms": 3}
                             for engine in ("kaveon", "trino")}}
             for name, sql in EXTENDED_QUERIES.items()]
    rounds = [{"round": index, "successful_qps": 10} for index in range(6)]
    return {"same_file_correctness": True, "files": {name: {"sha256": "b" * 64} for name in ("events", "customers")},
            "suite": "extended", "cases": cases,
            "query_corpus": {"sha256": hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()},
            "fair_performance_comparison": True,
            "kaveon_runtime_limits": {"Memory": 8}, "trino_runtime_limits": {"Memory": 8},
            "workers": 0, "trino_active_nodes": 1, "local_parallelism": 4, "warmups_per_query": 5,
            "cache_policy": {"primary": "warm", "cold_cache": "separate", "reason": "matched eviction required"},
            "throughput": {"concurrency": 4, "rounds": 6, "queries_per_round": 120,
                           "repetitions_per_query_per_round": 10, "kaveon": rounds, "trino": rounds,
                           "passed": True, "kaveon_over_trino": 1.9},
            "workspace_at_invocation": {"engine_source_manifest_sha256": "c" * 64},
            "docker_image": "sha256:k", "trino_image": "sha256:t"}


class TrinoClaimGateTests(unittest.TestCase):
    def test_valid_evidence_passes_technical_gate_but_does_not_publish_broad_claim(self):
        result = evaluate(valid_report())
        self.assertTrue(result["technical_gate_passed"])
        self.assertFalse(result["claim_eligible"])
        self.assertEqual(result["primary_metric"]["status"], "proposed_pending_user_acceptance")

    def test_wrong_result_or_ratio_below_target_fails_closed(self):
        report = valid_report(); report["cases"][0]["passed"] = False
        self.assertFalse(evaluate(report)["technical_gate_passed"])
        report = valid_report(); report["throughput"]["kaveon_over_trino"] = 1.899
        self.assertFalse(evaluate(report)["technical_gate_passed"])

    def test_short_unmatched_or_undeclared_cache_run_fails(self):
        report = copy.deepcopy(valid_report())
        report["cases"][0]["kaveon_ms"] = [1] * 29
        report["trino_runtime_limits"] = {"Memory": 4}
        report.pop("cache_policy")
        result = evaluate(report)
        self.assertFalse(result["technical_gate_passed"])
        self.assertIn("latency_samples", result["failed_checks"])
        self.assertIn("resource_limits_matched", result["failed_checks"])
        self.assertIn("warm_cache_primary", result["failed_checks"])


if __name__ == "__main__": unittest.main()
