"""Combine analytics and transaction evidence into a fail-closed comparison report.

This evaluator does not run either workload. It consumes the machine-readable
reports produced by independent, resource-matched runners and refuses to publish
a win unless correctness, workload scale, sample count, and resource gates pass.
Missing reports are recorded as pending so this command remains useful before
external PostgreSQL or Trino services are available.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path


ANALYTICS_TARGET = 1.9
REQUIRED_TRANSACTION_OPERATIONS = {
    "point_read", "insert", "update", "delete", "conflicting_update", "multi_record_commit"
}


def load(path):
    if path is None:
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def analytics_gate(report):
    reasons = []
    if report is None:
        return {"status": "pending", "qualified": False,
                "reasons": ["analytics report was not supplied"]}
    cases = report.get("cases") or []
    throughput = report.get("throughput") or {}
    checks = {
        "same_files": report.get("same_file_correctness") is True,
        "resources_matched": report.get("fair_performance_comparison") is True,
        "publication_workload": report.get("publication_workload_gate") is True,
        "all_results_exact": bool(cases) and all(case.get("passed") is True and case.get("result_sha256") for case in cases),
        "throughput_correct": throughput.get("passed") is True,
        "target_ratio": isinstance(throughput.get("kaveon_over_trino"), (int, float)) and throughput["kaveon_over_trino"] >= ANALYTICS_TARGET,
    }
    reasons.extend(name.replace("_", " ") + " gate failed" for name, passed in checks.items() if not passed)
    qualified = all(checks.values())
    return {"status": "qualified" if qualified else "not_qualified", "qualified": qualified,
            "checks": checks, "target": {"metric": "successful exact-result queries per second", "kaveon_over_trino": ANALYTICS_TARGET},
            "observed_ratio": throughput.get("kaveon_over_trino"), "reasons": reasons}


def transaction_gate(report):
    reasons = []
    if report is None:
        return {"status": "pending", "qualified": False,
                "reasons": ["transaction report was not supplied"]}
    operations = report.get("operations") or []
    names = {item.get("name") for item in operations}
    missing = sorted(REQUIRED_TRANSACTION_OPERATIONS - names)
    checks = {
        "resources_matched": report.get("resources_matched") is True,
        "publication_workload": report.get("publication_workload_gate") is True,
        "correctness": report.get("correctness_passed") is True,
        "required_operations": not missing,
        "operation_samples": bool(operations) and all(item.get("samples", 0) >= 30 for item in operations),
        "operation_results": bool(operations) and all(item.get("passed") is True and item.get("state_sha256") for item in operations),
        "throughput_win": isinstance(report.get("kaveon_over_postgresql_qps"), (int, float)) and report["kaveon_over_postgresql_qps"] > 1.0,
        "tail_latency_win": isinstance(report.get("kaveon_over_postgresql_p95_ratio"), (int, float)) and report["kaveon_over_postgresql_p95_ratio"] < 1.0,
    }
    reasons.extend(name.replace("_", " ") + " gate failed" for name, passed in checks.items() if not passed)
    if missing:
        reasons.append("missing operations: " + ", ".join(missing))
    qualified = all(checks.values())
    return {"status": "qualified" if qualified else "not_qualified", "qualified": qualified,
            "checks": checks,
            "target": {"throughput": "Kaveon QPS > PostgreSQL QPS", "tail_latency": "Kaveon p95 < PostgreSQL p95"},
            "observed": {"qps_ratio": report.get("kaveon_over_postgresql_qps"), "p95_ratio": report.get("kaveon_over_postgresql_p95_ratio")},
            "reasons": reasons}


def evaluate(analytics, transactions):
    analytical = analytics_gate(analytics)
    transactional = transaction_gate(transactions)
    qualified = analytical["qualified"] and transactional["qualified"]
    return {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "claim": "qualified_for_declared_workloads" if qualified else "not_qualified",
        "qualified": qualified,
        "analytics_vs_trino": analytical,
        "transactions_vs_postgresql": transactional,
        "scope": "Only the declared matched datasets, workloads, versions, resource limits, cache policy, and concurrency represented by the input reports.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--analytics", type=Path, help="same_files.py report.json")
    parser.add_argument("--transactions", type=Path, help="transaction runner report.json")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = evaluate(load(args.analytics), load(args.transactions))
    result["inputs"] = {
        name: ({"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
               if path is not None else None)
        for name, path in (("analytics", args.analytics), ("transactions", args.transactions))
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"{result['claim']}: {args.output}")
    return 0 if result["qualified"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
