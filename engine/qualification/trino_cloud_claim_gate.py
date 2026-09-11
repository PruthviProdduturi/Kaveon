"""Fail-closed evaluator for the distributed AKS Kaveon/Trino evidence bundle."""

import argparse
import hashlib
import json
from pathlib import Path

from same_files import EXTENDED_QUERIES


TARGET = 1.90


def evaluate(report):
    manifest = report.get("manifest") or {}
    dataset = manifest.get("dataset") or {}
    corpus = manifest.get("query_corpus") or {}
    cases = report.get("cases") or []
    throughput = report.get("throughput") or {}
    policy = report.get("policy") or {}
    preflight = report.get("preflight") or {}
    expected_hash = hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    expected_names = set(EXTENDED_QUERIES)
    verified = report.get("verified_blobs") or []
    observations = report.get("co_tenant_observations") or []
    co_tenant_baseline = report.get("co_tenant_baseline")
    data_objects = [item for item in dataset.get("objects") or [] if item.get("parquet_data")]
    expected_objects = {(item.get("path"), item.get("sha256"), item.get("content_md5"), item.get("bytes")) for item in dataset.get("objects") or []}
    verified_objects = {(item.get("path"), item.get("sha256"), item.get("content_md5"), item.get("bytes")) for item in verified}
    checks = {
        "three_worker_matched_topology": report.get("workers") == 3
            and preflight.get("checks", {}).get("one_system_three_worker_nodes") is True
            and all(len(set(sample.get("worker_nodes") or [])) == 3
                    for engine in ("kaveon", "trino") for sample in throughput.get(engine) or []),
        "stable_recorded_co_tenants": isinstance(co_tenant_baseline, list) and len(observations) == 12
            and sum(item.get("engine") == "kaveon" for item in observations) == 6
            and sum(item.get("engine") == "trino" for item in observations) == 6
            and all(item.get("co_tenants") == co_tenant_baseline for item in observations),
        "matched_role_resources": preflight.get("checks", {}).get("coordinator_resources_matched") is True
            and preflight.get("checks", {}).get("worker_resources_matched") is True,
        "immutable_images": preflight.get("checks", {}).get("kaveon_image_pinned_and_expected") is True
            and preflight.get("checks", {}).get("trino_image_pinned") is True
            and all(sample.get("worker_image_ids") and all("sha256:" in value for value in sample["worker_image_ids"])
                    for engine in ("kaveon", "trino") for sample in throughput.get(engine) or []),
        "tls_and_authentication_enforced": report.get("security_boundaries") == {"kaveon": True, "trino": True},
        "same_verified_parquet_objects": len(data_objects) == 2 and expected_objects == verified_objects
            and len(verified) == len(expected_objects) and all(item.get("sha256") and item.get("etag") for item in verified),
        "publication_dataset": dataset.get("rows", 0) >= 5_000_000 and dataset.get("customers", 0) >= 100_000,
        "exact_extended_corpus": corpus.get("sha256") == expected_hash
            and {case.get("name") for case in cases} == expected_names
            and all(case.get("sql") == EXTENDED_QUERIES.get(case.get("name")) for case in cases),
        "exact_results": bool(cases) and all(case.get("passed") is True and case.get("result_sha256")
            == (corpus.get("queries", {}).get(case.get("name")) or {}).get("result_sha256") for case in cases),
        "latency_samples": bool(cases) and all(len(case.get(engine + "_ms") or []) >= 30
            for case in cases for engine in ("kaveon", "trino")),
        "latency_percentiles": bool(cases) and all(all(key in case.get("statistics", {}).get(engine, {})
            for key in ("min_ms", "median_ms", "p95_ms", "max_ms")) for case in cases for engine in ("kaveon", "trino")),
        "frozen_workload_policy": policy == {"worker_count": 3, "warmups": 5, "repetitions_per_round": 5,
            "rounds": 6, "throughput_repeats": 10, "concurrency": 4, "target_ratio": 1.9},
        "alternating_complete_rounds": len(throughput.get("kaveon") or []) == 6 and len(throughput.get("trino") or []) == 6
            and all(sample.get("order") == (["trino", "kaveon"] if (sample.get("round", 0) - 1) % 2 == 0 else ["kaveon", "trino"])
                    for engine in ("kaveon", "trino") for sample in throughput.get(engine) or []),
        "throughput_correct": throughput.get("passed") is True,
        "ratio_at_least_1_90": isinstance(throughput.get("kaveon_over_trino"), (int, float))
            and not isinstance(throughput.get("kaveon_over_trino"), bool) and throughput["kaveon_over_trino"] >= TARGET,
        "kaveon_restored": not (report.get("restoration") or {}).get("errors"),
    }
    passed = all(checks.values())
    return {"schema_version": 1, "primary_metric": {"status": "proposed_pending_user_acceptance",
            "name": "successful exact-result queries per second", "workload": "extended 12-query AKS workload at concurrency 4",
            "target": TARGET, "observed": throughput.get("kaveon_over_trino")},
            "technical_gate_passed": passed, "claim_eligible": False,
            "claim_blocker": "The primary metric remains proposed; this gate cannot publish a broad Trino superiority claim.",
            "checks": checks, "failed_checks": [name for name, value in checks.items() if not value],
            "scope": "Three-worker, same-AKS-node-SKU, matched co-tenant warm-cache leases over the declared immutable ADLS fixture."}


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
