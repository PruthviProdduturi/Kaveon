"""Run a bounded, credential-safe mixed-workload soak against the current AKS Engine."""

from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time


QUALIFY = Path(__file__).with_name("qualify-aks-fault-pressure.py")
SPEC = importlib.util.spec_from_file_location("aks_qualify", QUALIFY)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def pod_inventory(kube):
    return MODULE.pod_inventory(kube)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", default="kaveon-aks")
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--duration-seconds", type=int, default=120)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--output", type=Path, default=Path("tmp/aks-current-soak/report.json"))
    args = parser.parse_args()
    if args.duration_seconds < 30 or not 1 <= args.concurrency <= 8:
        parser.error("duration must be at least 30 seconds and concurrency must be 1..8")

    kube = MODULE.Kube(args.context, args.namespace)
    report = {
        "passed": False,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "context": args.context,
        "namespace": args.namespace,
        "duration_seconds": args.duration_seconds,
        "concurrency": args.concurrency,
        "credential_material_recorded": False,
        "subscription_policy_changed": False,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    forward = None
    log = None
    try:
        before = pod_inventory(kube)
        workers = sorted(name for name in before if name.startswith("kaveon-worker-"))
        if len(workers) != 3 or not all(before[name]["ready"] for name in workers):
            raise RuntimeError("expected three Ready workers before soak")
        report["pods_before"] = before
        report["retained_files_before"] = {
            "coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange"),
            **{name + "_spill": kube.file_count(name, "/tmp/spill") for name in workers},
        }
        engine_secret = MODULE.engine_secret_data(kube)
        tls_secret = MODULE.secret_data(kube, MODULE.TLS_SECRET)
        token, principal = MODULE.choose_principal(base64.b64decode(engine_secret["security.json"]))
        report["principal"] = principal
        with tempfile.TemporaryDirectory(prefix="kaveon-aks-soak-") as private:
            ca_path = Path(private) / "ca.crt"
            ca_path.write_bytes(base64.b64decode(tls_secret["ca.crt"]))
            port = MODULE.free_port()
            log = (args.output.parent / "port-forward.log").open("w", encoding="utf-8")
            forward = subprocess.Popen(
                [*kube.prefix, "port-forward", "service/kaveon-coordinator", f"{port}:8080", "--address", "127.0.0.1"],
                stdout=log,
                stderr=subprocess.STDOUT,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            MODULE.wait_for_port(port, forward)
            engine = MODULE.Engine(f"https://127.0.0.1:{port}", token, str(ca_path))
            cases = [
                {"name": "dashboard_count", "catalog": "OpenSource", "schema": "public", "sql": "SELECT COUNT(*) FROM kaveon_events_dashboard", "expected": [[24]]},
                {"name": "borough_count", "catalog": "OpenSource", "schema": "public", "sql": "SELECT COUNT(*) FROM nyc_taxi_borough", "expected": [[5]]},
                {"name": "leaderboard_count", "catalog": "OpenSource", "schema": "ai_benchmarks", "sql": "SELECT COUNT(*) FROM leaderboard", "expected": [[34]]},
                {"name": "dashboard_group", "catalog": "OpenSource", "schema": "public", "sql": "SELECT region, COUNT(*), SUM(actions) FROM kaveon_events_dashboard GROUP BY region ORDER BY region", "expected": None},
            ]
            baseline = [engine.submit(case) for case in cases]
            cases[-1]["expected"] = baseline[-1]["data"]
            results = []
            deadline = time.monotonic() + args.duration_seconds
            round_number = 0
            while time.monotonic() < deadline:
                round_number += 1
                with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                    futures = [pool.submit(engine.submit, cases[index % len(cases)], include_data=True) for index in range(args.concurrency)]
                    for future in as_completed(futures):
                        try:
                            result = future.result()
                            expected = cases[next(i for i, f in enumerate(futures) if f is future) % len(cases)]["expected"]
                            result["exact"] = result.get("data") == expected
                            result["round"] = round_number
                            results.append(result)
                        except Exception as error:
                            results.append({"exact": False, "error": str(error), "round": round_number})
            report["requests"] = results
        after = pod_inventory(kube)
        report["pods_after"] = after
        report["retained_files_after"] = {
            "coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange"),
            **{name + "_spill": kube.file_count(name, "/tmp/spill") for name in workers},
        }
        report["request_count"] = len(results)
        report["exact_count"] = sum(bool(item.get("exact")) for item in results)
        report["failure_count"] = sum(not item.get("exact", False) for item in results)
        report["unexpected_restart_deltas"] = {
            name: after[name]["restart_count"] - before[name]["restart_count"]
            for name in before.keys() & after.keys()
            if after[name]["restart_count"] != before[name]["restart_count"]
        }
        report["checks"] = {
            "minimum_requests_completed": report["request_count"] >= args.duration_seconds,
            "all_results_exact": report["request_count"] > 0 and report["failure_count"] == 0,
            "workers_remained_ready": all(after.get(name, {}).get("ready") for name in workers),
            "no_unexpected_restarts": not report["unexpected_restart_deltas"],
            "retained_files_did_not_grow": all(
                report["retained_files_after"].get(name) is None
                or report["retained_files_before"].get(name) is None
                or report["retained_files_after"][name] <= report["retained_files_before"][name]
                for name in report["retained_files_after"]
            ),
        }
        report["passed"] = all(report["checks"].values())
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        if forward is not None and forward.poll() is None:
            forward.terminate()
            try:
                forward.wait(timeout=10)
            except subprocess.TimeoutExpired:
                forward.kill()
        if log is not None:
            log.close()
        report["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: AKS current soak report: {args.output}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
