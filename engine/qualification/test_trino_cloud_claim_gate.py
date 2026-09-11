import copy
import hashlib
import json
import unittest

from same_files import EXTENDED_QUERIES
from trino_cloud_claim_gate import evaluate


def valid_report():
    objects = [{"path": name, "sha256": "a" * 64, "content_md5": "bWQ1", "bytes": 1, "parquet_data": name.endswith("parquet")}
               for name in ("events/data.parquet", "events/log", "customers/data.parquet", "customers/log")]
    cases = [{"name": name, "sql": sql, "passed": True, "result_sha256": "b" * 64,
              "kaveon_ms": [1] * 30, "trino_ms": [2] * 30,
              "statistics": {engine: {key: 1 for key in ("min_ms", "median_ms", "p95_ms", "max_ms")}
                             for engine in ("kaveon", "trino")}} for name, sql in EXTENDED_QUERIES.items()]
    rounds = [{"round": index + 1, "order": ["trino", "kaveon"] if index % 2 == 0 else ["kaveon", "trino"],
               "worker_nodes": ["n1", "n2", "n3"], "worker_image_ids": ["image@sha256:" + "d" * 64]} for index in range(6)]
    return {"workers": 3, "manifest": {"dataset": {"rows": 5_000_000, "customers": 100_000, "objects": objects},
            "query_corpus": {"sha256": hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
                "queries": {name: {"result_sha256": "b" * 64} for name in EXTENDED_QUERIES}}},
            "preflight": {"checks": {"one_system_three_worker_nodes": True, "coordinator_resources_matched": True,
                "worker_resources_matched": True, "kaveon_image_pinned_and_expected": True, "trino_image_pinned": True}},
            "security_boundaries": {"kaveon": True, "trino": True},
            "verified_blobs": [dict(item, etag="e") for item in objects], "cases": cases,
            "policy": {"worker_count": 3, "warmups": 5, "repetitions_per_round": 5, "rounds": 6,
                "throughput_repeats": 10, "concurrency": 4, "target_ratio": 1.9},
            "throughput": {"kaveon": rounds, "trino": rounds, "passed": True, "kaveon_over_trino": 1.9},
            "restoration": {"errors": []}}


class CloudGateTests(unittest.TestCase):
    def test_complete_report_passes_technical_gate_only(self):
        result = evaluate(valid_report())
        self.assertTrue(result["technical_gate_passed"])
        self.assertFalse(result["claim_eligible"])

    def test_changed_blob_wrong_result_or_short_run_fails(self):
        report = valid_report()
        report["verified_blobs"][0]["sha256"] = ""
        report["cases"][0]["passed"] = False
        report["throughput"]["trino"].pop()
        failed = evaluate(report)["failed_checks"]
        self.assertIn("same_verified_parquet_objects", failed)
        self.assertIn("exact_results", failed)
        self.assertIn("alternating_complete_rounds", failed)


if __name__ == "__main__":
    unittest.main()
