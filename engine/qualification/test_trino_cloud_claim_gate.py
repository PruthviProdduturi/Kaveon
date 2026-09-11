import copy
import hashlib
import json
import unittest

from same_files import EXTENDED_QUERIES
from trino_cloud_claim_gate import evaluate


def valid_report():
    execution = {"0": {"samples": 1, "tasks_observed": 3, "tasks_with_metrics": 3,
        "tasks_with_cpu": 3, "compute_cpu_us": 10, "exchange_input_bytes": 20,
        "exchange_decode_bytes": 30, "exchange_decode_us": 4, "memory_peak_bytes": 40,
        "spill_bytes_written": 0, "spill_runs_written": 0, "spill_compactions": 0,
        "spill_compaction_input_bytes": 0, "object_metadata_cache_hits": 0,
        "memory_reservation_calls": 1, "memory_reservation_bytes": 40,
        "aggregate_input_rows": 10, "aggregate_groups_created": 2,
        "aggregate_distinct_values_admitted": 0}}
    objects = [{"path": name, "sha256": "a" * 64, "content_md5": "bWQ1", "bytes": 1, "parquet_data": name.endswith("parquet")}
               for name in ("events/data.parquet", "events/log", "customers/data.parquet", "customers/log")]
    cases = [{"name": name, "sql": sql, "passed": True, "result_sha256": "b" * 64,
              "kaveon_ms": [1] * 30, "trino_ms": [2] * 30,
              "kaveon_execution_by_stage": copy.deepcopy(execution),
              "statistics": {engine: {key: 1 for key in ("min_ms", "median_ms", "p95_ms", "max_ms")}
                             for engine in ("kaveon", "trino")}} for name, sql in EXTENDED_QUERIES.items()]
    rounds = [{"round": index + 1, "order": ["trino", "kaveon"] if index % 2 == 0 else ["kaveon", "trino"],
               "execution_by_stage": copy.deepcopy(execution),
               "worker_nodes": ["n1", "n2", "n3"], "worker_image_ids": ["image@sha256:" + "d" * 64]} for index in range(6)]
    return {"workers": 3, "manifest": {"dataset": {"rows": 5_000_000, "customers": 100_000, "objects": objects},
            "query_corpus": {"sha256": hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
                "queries": {name: {"result_sha256": "b" * 64} for name in EXTENDED_QUERIES}}},
            "preflight": {"checks": {"one_system_three_worker_nodes": True, "coordinator_resources_matched": True,
                "worker_resources_matched": True, "kaveon_image_pinned_and_expected": True, "trino_image_pinned": True}},
            "security_boundaries": {"kaveon": True, "trino": True},
            "co_tenant_baseline": ["n1/kube-system/coredns-a"],
            "co_tenant_observations": [
                {"engine": engine, "co_tenants": ["n1/kube-system/coredns-a"]}
                for _ in range(6) for engine in ("trino", "kaveon")
            ],
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

    def test_changed_co_tenant_topology_fails(self):
        report = valid_report()
        report["co_tenant_observations"][3]["co_tenants"] = []
        self.assertIn("stable_recorded_co_tenants", evaluate(report)["failed_checks"])

    def test_missing_or_partial_execution_metrics_fail(self):
        report = valid_report()
        del report["cases"][0]["kaveon_execution_by_stage"]
        self.assertIn("kaveon_execution_metrics_retained", evaluate(report)["failed_checks"])
        report = valid_report()
        report["throughput"]["kaveon"][0]["execution_by_stage"]["0"]["tasks_with_cpu"] = 2
        self.assertIn("kaveon_execution_metrics_retained", evaluate(report)["failed_checks"])


if __name__ == "__main__":
    unittest.main()
