"""Merge the reproducible AKS execution and fault-pressure reports.

The result is deliberately fail-closed: latency and throughput are retained as
diagnostics, while worker retry, coordinator cleanup, exact results, and
bounded resources are independent release gates.
"""

import argparse
import json
import math
from pathlib import Path


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * fraction) - 1] if ordered else None


def latency_summary(comparison):
    values = [sample for case in comparison.get("cases", [])
              for sample in case.get("kaveon_ms", [])]
    return {
        "samples": len(values),
        "p50_ms": percentile(values, 0.50),
        "p95_ms": percentile(values, 0.95),
        "p99_ms": percentile(values, 0.99),
        "max_ms": max(values) if values else None,
    }


def execution_metrics(comparison):
    totals = {
        "compute_cpu_us": 0,
        "compute_wall_us": 0,
        "exchange_input_bytes": 0,
        "exchange_output_bytes": 0,
        "exchange_output_copies": 0,
        "exchange_hash_us": 0,
        "exchange_copy_us": 0,
        "exchange_copy_allocations": 0,
        "exchange_copied_bytes": 0,
        "spill_bytes_written": 0,
        "spill_runs_written": 0,
        "spill_compactions": 0,
    }
    peaks = {"memory_peak_bytes": 0, "spill_peak_bytes": 0}
    for case in comparison.get("cases", []):
        for stage in case.get("kaveon_execution_by_stage", {}).values():
            for name in totals:
                totals[name] += stage.get(name, 0) or 0
            for name in peaks:
                peaks[name] = max(peaks[name], stage.get(name, 0) or 0)
    return {"totals": totals, "peaks": peaks}


def merge(comparison, pressure):
    concurrent = pressure.get("concurrent_exact", {})
    concurrent_requests = concurrent.get("requests", [])
    exact_requests = bool(concurrent_requests) and all(
        request.get("exact") is True and request.get("status") == 200
        for request in concurrent_requests
    )
    worker_loss = pressure.get("worker_loss", {})
    retry = bool(
        pressure.get("checks", {}).get("worker_loss_exact_retry_and_recovery")
        and worker_loss.get("exact") is True
        and worker_loss.get("retry", {}).get("attempt_incremented") is True
    )
    retained_after = pressure.get("retained_files_after", {})
    coordinator_cleanup = retained_after.get("coordinator_exchange") == 0
    worker_spill_cleanup = all(
        value == 0 for key, value in retained_after.items() if key.endswith("_spill")
    )
    metrics = execution_metrics(comparison)
    gates = {
        "exact_results": bool(comparison.get("passed")) and exact_requests,
        "concurrency": bool(comparison.get("throughput", {}).get("passed")) and exact_requests,
        "worker_retry": retry,
        "coordinator_restart_cleanup": coordinator_cleanup,
        "worker_spill_cleanup": worker_spill_cleanup,
        "memory_within_pod_limits": bool(
            pressure.get("pressure", {}).get("sampled_engine_memory_within_pod_limits")
        ),
    }
    return {
        "schema_version": 1,
        "status": "passed" if all(gates.values()) else "pending",
        "passed": all(gates.values()),
        "gates": gates,
        "latency": latency_summary(comparison),
        "throughput": comparison.get("throughput", {}),
        "execution_metrics": metrics,
        "admission": {
            "source": "comparison task evidence",
            "measured": any(
                stage.get("admission_wait_us", 0) > 0
                for case in comparison.get("cases", [])
                for stage in case.get("kaveon_execution_by_stage", {}).values()
            ),
        },
        "worker_retry": {
            "passed": retry,
            "source": "fault-pressure report",
        },
        "coordinator_restart_cleanup": {
            "passed": coordinator_cleanup,
            "retained_files_after": retained_after,
        },
        "known_open_gates": [
            "hash aggregate and hash join spill must remain bounded under forced pressure",
            "coordinator restart cleanup requires zero retained exchange chunks",
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--comparison", type=Path, required=True)
    parser.add_argument("--fault-pressure", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = merge(json.loads(args.comparison.read_text()),
                   json.loads(args.fault_pressure.read_text()))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"distributed qualification: {result['status']}")
    return 0 if result["passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
