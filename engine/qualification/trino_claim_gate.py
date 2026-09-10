"""Fail-closed evaluator for the proposed 1.90x Kaveon/Trino throughput metric."""
import argparse
import hashlib
import json
from pathlib import Path

from same_files import EXTENDED_QUERIES


TARGET = 1.90


def evaluate(report):
    cases = report.get("cases") or []
    throughput = report.get("throughput") or {}
    cache = report.get("cache_policy") or {}
    expected_names = set(EXTENDED_QUERIES)
    expected_corpus_hash = hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    actual_names = {case.get("name") for case in cases}
    repetitions = min((len(case.get("kaveon_ms") or []) for case in cases), default=0)
    trino_repetitions = min((len(case.get("trino_ms") or []) for case in cases), default=0)
    checks = {
        "same_persisted_files": report.get("same_file_correctness") is True
            and len(report.get("files") or {}) == 2
            and all(item.get("sha256") for item in (report.get("files") or {}).values()),
        "exact_extended_corpus": report.get("suite") == "extended" and actual_names == expected_names
            and all(case.get("sql") == EXTENDED_QUERIES.get(case.get("name")) for case in cases)
            and report.get("query_corpus", {}).get("sha256") == expected_corpus_hash,
        "correct_results": bool(cases) and all(case.get("passed") is True and case.get("result_sha256") for case in cases),
        "resource_limits_matched": report.get("fair_performance_comparison") is True
            and report.get("kaveon_runtime_limits") == report.get("trino_runtime_limits"),
        "single_node_topology_matched": report.get("workers") == 0
            and report.get("trino_active_nodes") == 1 and report.get("local_parallelism") == 4,
        "warm_cache_primary": cache.get("primary") == "warm" and report.get("warmups_per_query", 0) >= 5,
        "cold_cache_policy_declared": isinstance(cache.get("cold_cache"), str) and bool(cache.get("reason")),
        "latency_samples": repetitions >= 30 and trino_repetitions >= 30,
        "latency_percentiles": bool(cases) and all(
            engine in (case.get("median_ms") or {})
            and all(key in (case.get("statistics") or {}).get(engine, {}) for key in ("min_ms", "p95_ms", "max_ms"))
            for case in cases for engine in ("kaveon", "trino")),
        "matched_concurrency": throughput.get("concurrency") == 4,
        "alternating_throughput_rounds": throughput.get("rounds", 0) >= 6
            and throughput.get("repetitions_per_query_per_round") == 10
            and throughput.get("queries_per_round") == len(EXTENDED_QUERIES) * 10
            and len(throughput.get("kaveon") or []) == throughput.get("rounds")
            and len(throughput.get("trino") or []) == throughput.get("rounds"),
        "throughput_correct": throughput.get("passed") is True,
        "ratio_at_least_1_90": isinstance(throughput.get("kaveon_over_trino"), (int, float))
            and not isinstance(throughput.get("kaveon_over_trino"), bool)
            and throughput["kaveon_over_trino"] >= TARGET,
        "source_and_runtime_provenance": bool(report.get("workspace_at_invocation", {}).get("engine_source_manifest_sha256"))
            and bool(report.get("docker_image")) and bool(report.get("trino_image")),
    }
    technically_met = all(checks.values())
    return {
        "schema_version": 1,
        "primary_metric": {
            "status": "proposed_pending_user_acceptance",
            "name": "successful exact-result queries per second",
            "workload": "equal-weight extended 12-query mixed workload at concurrency 4",
            "target": TARGET,
            "observed": throughput.get("kaveon_over_trino"),
        },
        "technical_gate_passed": technically_met,
        "claim_eligible": False,
        "claim_blocker": "Primary metric is proposed and has not been recorded as user-accepted; this evaluator never publishes the broad phrase '90% better than Trino'.",
        "checks": checks,
        "failed_checks": [name for name, passed in checks.items() if not passed],
        "scope": "Single-node, equal-resource, warm-cache, client-observed performance on the declared corpus only. Cold-cache and matched distributed-cluster results are separate evidence.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = evaluate(json.loads(args.report.read_text(encoding="utf-8")))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"technical_gate_passed={str(result['technical_gate_passed']).lower()}; metric_status=proposed")
    return 0 if result["technical_gate_passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
