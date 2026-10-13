"""Qualify fail-closed behavior when the AKS coordinator is restarted mid-query."""

from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time


SOURCE = Path(__file__).with_name("qualify-aks-fault-pressure.py")
SPEC = importlib.util.spec_from_file_location("aks_qualify", SOURCE)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", default="kaveon-aks")
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--output", type=Path, default=Path("tmp/aks-coordinator-loss/report.json"))
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    kube = MODULE.Kube(args.context, args.namespace)
    report = {
        "passed": False,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "context": args.context,
        "namespace": args.namespace,
        "credential_material_recorded": False,
        "subscription_policy_changed": False,
        "scope": "Fail-closed active-query interruption during coordinator restart; no survival claim.",
    }
    forward = None
    log = None
    try:
        before = MODULE.pod_inventory(kube)
        report["pods_before"] = before
        report["retained_files_before"] = {"coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange")}
        engine_secret = MODULE.engine_secret_data(kube)
        tls_secret = MODULE.secret_data(kube, MODULE.TLS_SECRET)
        token, principal = MODULE.choose_principal(base64.b64decode(engine_secret["security.json"]))
        report["principal"] = principal
        with tempfile.TemporaryDirectory(prefix="kaveon-aks-coordinator-loss-") as private:
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
            case = {
                "name": "coordinator_loss_scan",
                "catalog": "OpenSource",
                "schema": "public",
                "sql": "SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0",
            }
            prior = engine.get("/v1/query")
            prior_ids = {item.get("id") for item in (prior.get("queries", []) if isinstance(prior, dict) else prior)}
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(engine.submit, case, include_data=True, timeout=300)
                query_id, live = MODULE.wait_for_query(engine, prior_ids, case["sql"], timeout=60)
                report["query_id"] = query_id
                report["query_observed_running"] = live.get("state") == "RUNNING"
                if not report["query_observed_running"]:
                    raise RuntimeError(f"query did not reach RUNNING: {live.get('state')}")
                subprocess.run(
                    [*kube.prefix, "rollout", "restart", "statefulset/kaveon-coordinator"],
                    capture_output=True,
                    text=True,
                    check=True,
                    timeout=30,
                )
                restart = subprocess.run(
                    [*kube.prefix, "rollout", "status", "statefulset/kaveon-coordinator", "--timeout=180s"],
                    capture_output=True,
                    text=True,
                    check=True,
                    timeout=210,
                )
                report["restart_status"] = restart.stdout.strip()
                try:
                    result = future.result(timeout=300)
                    report["interrupted_request"] = {"state": result.get("state"), "status": result.get("status"), "error": result.get("error")}
                    report["interruption_observed"] = result.get("state") != "FINISHED" or bool(result.get("error"))
                except Exception as error:
                    report["interrupted_request"] = {"exception_type": type(error).__name__, "error": str(error)}
                    report["interruption_observed"] = True
            # Coordinator restart tears down the old port-forward stream. Use
            # a fresh loopback forward for the post-restart probe.
            if forward is not None and forward.poll() is None:
                forward.terminate()
                try:
                    forward.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    forward.kill()
            if log is not None:
                log.close()
            port = MODULE.free_port()
            log = (args.output.parent / "port-forward-recovery.log").open("w", encoding="utf-8")
            forward = subprocess.Popen(
                [*kube.prefix, "port-forward", "service/kaveon-coordinator", f"{port}:8080", "--address", "127.0.0.1"],
                stdout=log,
                stderr=subprocess.STDOUT,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            MODULE.wait_for_port(port, forward)
            engine = MODULE.Engine(f"https://127.0.0.1:{port}", token, str(ca_path))
            time.sleep(2)
            after = MODULE.pod_inventory(kube)
            report["pods_after"] = after
            report["retained_files_after"] = {"coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange")}
            probe = engine.submit({"name": "post_coordinator_restart_probe", "catalog": "OpenSource", "schema": "public", "sql": "SELECT COUNT(*) FROM kaveon_events_dashboard"})
            report["post_restart_probe"] = {"state": probe.get("state"), "data": probe.get("data"), "exact": probe.get("data") == [[24]]}
        report["checks"] = {
            "query_observed_running": report.get("query_observed_running", False),
            "interruption_observed": report.get("interruption_observed", False),
            "coordinator_ready_after_restart": after.get("kaveon-coordinator-0", {}).get("ready", False),
            "three_workers_ready_after_restart": sum(1 for name in after if name.startswith("kaveon-worker-") and after[name]["ready"]) == 3,
            "post_restart_exact_probe": report["post_restart_probe"]["exact"],
            "no_retained_exchange_growth": report["retained_files_after"]["coordinator_exchange"] <= report["retained_files_before"]["coordinator_exchange"],
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
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: AKS coordinator-loss report: {args.output}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
