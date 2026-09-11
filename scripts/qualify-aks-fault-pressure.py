"""Credential-safe AKS worker-loss, concurrency, and pressure qualification.

The verifier reads the Engine principal and CA from Kubernetes Secrets into
memory, opens a loopback-only port-forward, and never writes credentials to its
report. It deletes exactly one StatefulSet worker during an observed distributed
join, waits for the replacement to become catalog-compatible, then runs bounded
concurrent exact-result pressure at the configured concurrency.
"""

from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import threading
import time
from typing import Any

import requests
from requests.adapters import HTTPAdapter


TLS_HOSTNAME = "kaveon-coordinator.kaveon.svc.cluster.local"
ENGINE_SECRET = "kaveon-coordinator-auth"
TLS_SECRET = "kaveon-engine-tls"
YELLOW_ROWS = 3_412_043


class EngineTlsAdapter(HTTPAdapter):
    """Connect to the forwarded IP while validating the cluster DNS SAN."""

    def init_poolmanager(self, *args: Any, **kwargs: Any) -> None:
        kwargs["assert_hostname"] = TLS_HOSTNAME
        super().init_poolmanager(*args, **kwargs)


def run(command: list[str], *, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, capture_output=True, text=True, check=True, timeout=timeout)


class Kube:
    def __init__(self, context: str, namespace: str) -> None:
        self.prefix = ["kubectl", "--context", context, "-n", namespace]

    def json(self, kind: str, name: str | None = None) -> Any:
        command = [*self.prefix, "get", kind]
        if name:
            command.append(name)
        command.extend(["-o", "json"])
        return json.loads(run(command).stdout)

    def delete_worker(self, name: str) -> None:
        pod = self.json("pod", name)
        owners = pod["metadata"].get("ownerReferences", [])
        if not any(owner.get("kind") == "StatefulSet" and owner.get("name") == "kaveon-worker" for owner in owners):
            raise AssertionError(f"Refusing to delete {name}: it is not owned by StatefulSet/kaveon-worker")
        # A normal Kubernetes deletion grants the process enough time to finish
        # most fixture queries. Force termination so this is a real worker-loss
        # test rather than a graceful rolling-restart test.
        run(
            [*self.prefix, "delete", "pod", name, "--grace-period=0", "--force", "--wait=false"],
            timeout=30,
        )

    def file_count(self, pod: str, root: str) -> int | None:
        command = [
            *self.prefix,
            "exec",
            pod,
            "--",
            "sh",
            "-c",
            f"if [ -d {root} ]; then find {root} -type f 2>/dev/null | wc -l; else echo 0; fi",
        ]
        try:
            return int(run(command, timeout=30).stdout.strip())
        except (subprocess.SubprocessError, ValueError):
            return None


def secret_data(kube: Kube, name: str) -> dict[str, str]:
    return kube.json("secret", name)["data"]


def choose_principal(security_json: bytes) -> tuple[str, str]:
    security = json.loads(security_json)
    eligible = [
        principal
        for principal in security.get("principals", [])
        if principal.get("role") in {"admin", "analyst"} and principal.get("token")
    ]
    if not eligible:
        raise RuntimeError("Engine Secret has no static admin or analyst principal")
    principal = eligible[0]
    return principal["token"], principal.get("principal", "unknown")


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_for_port(port: int, process: subprocess.Popen[Any], timeout: int = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("kubectl port-forward exited before becoming ready")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError:
            time.sleep(0.2)
    raise TimeoutError("kubectl port-forward did not become ready")


def canonical_hash(value: Any) -> str:
    payload = json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()
    return hashlib.sha256(payload).hexdigest()


def pod_inventory(kube: Kube) -> dict[str, dict[str, Any]]:
    inventory: dict[str, dict[str, Any]] = {}
    for pod in kube.json("pods")["items"]:
        name = pod["metadata"]["name"]
        if not name.startswith("kaveon-"):
            continue
        container = pod["spec"]["containers"][0]
        status = (pod.get("status", {}).get("containerStatuses") or [{}])[0]
        inventory[name] = {
            "uid": pod["metadata"]["uid"],
            "image": container.get("image"),
            "image_id": status.get("imageID"),
            "restart_count": status.get("restartCount", 0),
            "ready": status.get("ready", False),
            "node": pod["spec"].get("nodeName"),
            "resources": container.get("resources", {}),
        }
    return inventory


def parse_cpu(value: str) -> int:
    return int(value[:-1]) if value.endswith("m") else int(float(value) * 1000)


def parse_memory(value: str) -> int:
    units = {"Ki": 1024, "Mi": 1024**2, "Gi": 1024**3}
    for suffix, multiplier in units.items():
        if value.endswith(suffix):
            return int(float(value[: -len(suffix)]) * multiplier)
    return int(value)


def pressure_peaks(samples: list[dict[str, Any]]) -> dict[str, dict[str, int]]:
    peaks: dict[str, dict[str, int]] = {}
    for sample in samples:
        for pod, metrics in sample.get("pods", {}).items():
            peak = peaks.setdefault(pod, {"cpu_millicores": 0, "memory_bytes": 0})
            peak["cpu_millicores"] = max(peak["cpu_millicores"], metrics["cpu_millicores"])
            peak["memory_bytes"] = max(peak["memory_bytes"], metrics["memory_bytes"])
    return peaks


def top_sample(kube: Kube) -> dict[str, dict[str, Any]]:
    result = run([*kube.prefix, "top", "pods", "--no-headers"], timeout=30)
    sample: dict[str, dict[str, Any]] = {}
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 3 and fields[0].startswith("kaveon-"):
            sample[fields[0]] = {
                "cpu": fields[1],
                "cpu_millicores": parse_cpu(fields[1]),
                "memory": fields[2],
                "memory_bytes": parse_memory(fields[2]),
            }
    return sample


class Engine:
    def __init__(self, base: str, token: str, ca_path: str) -> None:
        self.base = base
        self.token = token
        self.ca_path = ca_path

    def session(self) -> requests.Session:
        session = requests.Session()
        session.mount("https://", EngineTlsAdapter())
        session.headers.update({"Authorization": "Bearer " + self.token})
        return session

    def get(self, path: str, *, timeout: int = 30) -> Any:
        with self.session() as session:
            response = session.get(self.base + path, verify=self.ca_path, timeout=timeout)
            response.raise_for_status()
            return response.json()

    def submit(self, case: dict[str, Any], *, include_data: bool = True, timeout: int = 240) -> dict[str, Any]:
        started = time.monotonic()
        payload = {
            "query": case["sql"],
            "catalog": case.get("catalog", "OpenSource"),
            "schema": case["schema"],
            "result_delivery": "inline",
            "client": case.get("client", "aks-fault-pressure-qualification"),
        }
        with self.session() as session:
            response = session.post(self.base + "/v1/statement", json=payload, verify=self.ca_path, timeout=timeout)
            body = response.json()
        record: dict[str, Any] = {
            "name": case["name"],
            "sql": case["sql"],
            "status": response.status_code,
            "id": body.get("id"),
            "state": body.get("state"),
            "error": body.get("error"),
            "elapsed_ms": body.get("elapsed_ms"),
            "client_elapsed_ms": round((time.monotonic() - started) * 1000, 2),
            "result_sha256": canonical_hash(body.get("data")),
            "row_count": len(body.get("data") or []),
        }
        if include_data:
            record["data"] = body.get("data")
        if response.status_code != 200 or body.get("state") != "FINISHED" or body.get("error"):
            raise AssertionError(json.dumps(record, separators=(",", ":")))
        if record["id"]:
            history = self.get("/v1/query/" + record["id"])
            stages = history.get("stages") or []
            tasks = [task for stage in stages for task in stage.get("tasks", [])]
            record["catalog_snapshot_id"] = (history.get("context") or {}).get("catalog_snapshot_id")
            record["stage_count"] = len(stages)
            record["task_count"] = len(tasks)
            record["worker_ids"] = sorted({task.get("node_id") for task in tasks if task.get("node_id")})
            record["task_ids"] = [task.get("task_id") for task in tasks]
            record["tasks"] = [
                {
                    "stage_id": stage.get("stage_id"),
                    "task_id": task.get("task_id"),
                    "node_id": task.get("node_id"),
                    "elapsed_us": task.get("elapsed_us"),
                }
                for stage in stages
                for task in stage.get("tasks", [])
            ]
            record["scan_metrics_complete"] = history.get("scan_metrics_complete")
        return record


def cluster_summary(cluster: dict[str, Any]) -> dict[str, Any]:
    return {
        "environment": cluster.get("environment"),
        "required_catalog_snapshot_id": cluster.get("required_catalog_snapshot_id"),
        "active_workers": cluster.get("active_workers"),
        "compatible_workers": cluster.get("compatible_workers"),
        "total_nodes": cluster.get("total_nodes"),
        "workers": [
            {
                "node_id": worker.get("node_id"),
                "catalog_snapshot_id": worker.get("catalog_snapshot_id"),
                "memory_rss_bytes": worker.get("memory_rss_bytes"),
            }
            for worker in cluster.get("workers", [])
        ],
    }


def wait_for_query(engine: Engine, prior_ids: set[str], sql: str, timeout: int = 30) -> tuple[str, dict[str, Any]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        records = engine.get("/v1/query")
        if isinstance(records, dict):
            records = records.get("queries", [])
        matches = [record for record in records if record.get("id") not in prior_ids and record.get("sql") == sql]
        if matches:
            query_id = matches[0]["id"]
            return query_id, engine.get("/v1/query/" + query_id)
        time.sleep(0.1)
    raise TimeoutError("Did not observe the fault query in query history")


def task_worker(record: dict[str, Any]) -> str | None:
    for stage in record.get("stages") or []:
        for task in stage.get("tasks", []):
            worker = task.get("node_id")
            if worker and worker.startswith("kaveon-worker-"):
                return worker
    return None


def wait_for_worker_task(engine: Engine, query_id: str, timeout: int = 10) -> tuple[str | None, dict[str, Any]]:
    deadline = time.monotonic() + timeout
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        latest = engine.get("/v1/query/" + query_id)
        worker = task_worker(latest)
        if worker:
            return worker, latest
        if latest.get("state") not in {"PLANNING", "QUEUED", "RUNNING"}:
            break
        time.sleep(0.1)
    return None, latest


def wait_for_recovery(kube: Kube, engine: Engine, worker: str, old_uid: str, catalog_id: str, timeout: int = 300) -> tuple[dict[str, Any], dict[str, Any]]:
    deadline = time.monotonic() + timeout
    last_cluster: dict[str, Any] = {}
    while time.monotonic() < deadline:
        try:
            pod = kube.json("pod", worker)
            statuses = pod.get("status", {}).get("containerStatuses") or []
            ready = bool(statuses and statuses[0].get("ready"))
            replaced = pod["metadata"]["uid"] != old_uid
            last_cluster = engine.get("/v1/cluster")
            compatible = (
                last_cluster.get("active_workers") == 3
                and last_cluster.get("compatible_workers") == 3
                and all(item.get("catalog_snapshot_id") == catalog_id for item in last_cluster.get("workers", []))
            )
            if ready and replaced and compatible:
                return pod, last_cluster
        except (subprocess.SubprocessError, requests.RequestException, KeyError):
            pass
        time.sleep(1)
    raise TimeoutError("Worker replacement did not return as one of three catalog-compatible workers")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", default="kaveon-test-aks")
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--output", type=Path, default=Path("tmp/aks-fault-pressure/report.json"))
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--concurrent-rounds", type=int, default=3)
    parser.add_argument("--pressure-rounds", type=int, default=3)
    args = parser.parse_args()
    if not 2 <= args.concurrency <= 8:
        parser.error("concurrency must be between 2 and 8")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report: dict[str, Any] = {
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "context": args.context,
        "namespace": args.namespace,
        "credential_material_recorded": False,
        "subscription_policy_changed": False,
        "checks": {},
        "passed": False,
    }
    kube = Kube(args.context, args.namespace)
    port_forward: subprocess.Popen[Any] | None = None
    port_log = None
    try:
        current_context = run(["kubectl", "config", "current-context"]).stdout.strip()
        report["shell_current_context"] = current_context
        report["explicit_target_context"] = args.context
        before_pods = pod_inventory(kube)
        workers = sorted(name for name in before_pods if re.fullmatch(r"kaveon-worker-\d+", name))
        if len(workers) != 3 or not all(before_pods[name]["ready"] for name in workers):
            raise AssertionError("Expected exactly three Ready kaveon-worker StatefulSet pods")
        report["pods_before"] = before_pods
        report["retained_files_before"] = {
            "coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange"),
            **{name + "_spill": kube.file_count(name, "/tmp/spill") for name in workers},
        }

        engine_secret = secret_data(kube, ENGINE_SECRET)
        tls_secret = secret_data(kube, TLS_SECRET)
        token, principal = choose_principal(base64.b64decode(engine_secret["security.json"]))
        report["principal"] = principal
        with tempfile.TemporaryDirectory(prefix="kaveon-aks-qualification-") as private:
            ca_path = Path(private) / "ca.crt"
            ca_path.write_bytes(base64.b64decode(tls_secret["ca.crt"]))
            port = free_port()
            port_log = (args.output.parent / "port-forward.log").open("w", encoding="utf-8")
            port_forward = subprocess.Popen(
                [*kube.prefix, "port-forward", "service/kaveon-coordinator", f"{port}:8080", "--address", "127.0.0.1"],
                stdout=port_log,
                stderr=subprocess.STDOUT,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            wait_for_port(port, port_forward)
            engine = Engine(f"https://127.0.0.1:{port}", token, str(ca_path))
            baseline_cluster = engine.get("/v1/cluster")
            catalog_id = baseline_cluster.get("required_catalog_snapshot_id")
            cluster_ok = (
                baseline_cluster.get("active_workers") == 3
                and baseline_cluster.get("compatible_workers") == 3
                and catalog_id
                and all(worker.get("catalog_snapshot_id") == catalog_id for worker in baseline_cluster.get("workers", []))
            )
            report["cluster_before"] = cluster_summary(baseline_cluster)
            report["checks"]["three_catalog_compatible_workers_before"] = bool(cluster_ok)
            if not cluster_ok:
                raise AssertionError("Baseline cluster is not three-worker catalog-compatible")

            exact_cases = [
                {"name": "yellow_count", "schema": "nyc_taxi", "sql": "SELECT COUNT(*) FROM yellow_trips", "expected": [[YELLOW_ROWS]]},
                {"name": "green_count", "schema": "nyc_taxi", "sql": "SELECT COUNT(*) FROM green_trips", "expected": [[48_131]]},
                {"name": "daily_totals", "schema": "nyc_taxi", "sql": "SELECT COUNT(*), SUM(trip_count), SUM(total_amount_cents) FROM daily_trips", "expected": [[62, 3_460_174, 9_175_357_418]]},
                {"name": "leaderboard_count", "schema": "ai_benchmarks", "sql": "SELECT COUNT(*) FROM leaderboard", "expected": [[34]]},
            ]
            concurrent_results: list[dict[str, Any]] = []
            for round_index in range(args.concurrent_rounds):
                with ThreadPoolExecutor(max_workers=args.concurrency) as executor:
                    futures = {executor.submit(engine.submit, case): case for case in exact_cases}
                    for future in as_completed(futures):
                        result = future.result()
                        result["round"] = round_index + 1
                        result["exact"] = result.get("data") == futures[future]["expected"]
                        concurrent_results.append(result)
            report["concurrent_exact"] = {
                "concurrency": args.concurrency,
                "rounds": args.concurrent_rounds,
                "requests": concurrent_results,
            }
            concurrent_ok = len(concurrent_results) == len(exact_cases) * args.concurrent_rounds and all(
                item["exact"] and item.get("stage_count", 0) >= 2 and item.get("worker_ids") for item in concurrent_results
            )
            report["checks"]["concurrent_exact_results"] = concurrent_ok
            if not concurrent_ok:
                raise AssertionError("Concurrent exact-result gate failed")

            fault_case = {
                "name": "yellow_zone_join_fault",
                "schema": "nyc_taxi",
                "sql": "SELECT COUNT(*), SUM(y.VendorID) FROM yellow_trips y JOIN taxi_zones z ON y.PULocationID = z.LocationID",
            }
            fault_baseline = engine.submit(fault_case)
            if not fault_baseline.get("data") or fault_baseline["data"][0][0] <= 0:
                raise AssertionError("Fault baseline join did not return a positive exact count")
            prior = engine.get("/v1/query")
            if isinstance(prior, dict):
                prior = prior.get("queries", [])
            prior_ids = {item.get("id") for item in prior}
            longest_baseline_task = max(
                fault_baseline["tasks"], key=lambda task: task.get("elapsed_us") or 0
            )
            target = longest_baseline_task["node_id"]
            if target not in workers:
                raise AssertionError("Fault baseline did not identify a StatefulSet worker task")
            with ThreadPoolExecutor(max_workers=1) as executor:
                future = executor.submit(engine.submit, fault_case)
                query_id, live_record = wait_for_query(engine, prior_ids, fault_case["sql"])
                # Baseline stage timing identifies a roughly 30-second join
                # task. Waiting five seconds puts the repeated query inside
                # that stage before the force deletion.
                time.sleep(5)
                live_record = engine.get("/v1/query/" + query_id)
                if live_record.get("state") != "RUNNING":
                    raise AssertionError("Fault query left RUNNING before the worker-loss barrier")
                old_uid = before_pods[target]["uid"]
                kube.delete_worker(target)
                fault_result = future.result(timeout=240)
            replacement, recovered_cluster = wait_for_recovery(kube, engine, target, old_uid, catalog_id)
            attempts = []
            for task_id in fault_result.get("task_ids", []):
                match = re.search(r"\.(\d+)$", task_id or "")
                if match:
                    attempts.append(int(match.group(1)))
            retry_observed = any(attempt > 0 for attempt in attempts)
            report["worker_loss"] = {
                "target": target,
                "old_uid": old_uid,
                "replacement_uid": replacement["metadata"]["uid"],
                "query_observed_running": live_record.get("state") == "RUNNING",
                "target_selected_from_longest_baseline_task": longest_baseline_task,
                "forced_termination_after_running_ms": 5_000,
                "retry_attempt_observed": retry_observed,
                "baseline": fault_baseline,
                "during_loss": fault_result,
                "exact_match": fault_result.get("data") == fault_baseline.get("data"),
                "cluster_after_recovery": cluster_summary(recovered_cluster),
            }
            fault_ok = (
                fault_result.get("data") == fault_baseline.get("data")
                and replacement["metadata"]["uid"] != old_uid
                and recovered_cluster.get("active_workers") == 3
                and recovered_cluster.get("compatible_workers") == 3
                and retry_observed
            )
            report["checks"]["worker_loss_exact_retry_and_recovery"] = fault_ok
            if not fault_ok:
                raise AssertionError("Worker-loss query did not prove exact retry and full recovery")

            recovery_probe = engine.submit(exact_cases[1])
            recovery_probe["exact"] = recovery_probe.get("data") == exact_cases[1]["expected"]
            report["recovery_probe"] = recovery_probe
            if not recovery_probe["exact"]:
                raise AssertionError("Post-recovery exact probe failed")

            pressure_case = {
                "name": "yellow_group_pressure",
                "schema": "nyc_taxi",
                "sql": "SELECT PULocationID, COUNT(*), SUM(VendorID) FROM yellow_trips GROUP BY PULocationID ORDER BY PULocationID",
            }
            pressure_baseline = engine.submit(pressure_case, include_data=True)
            if sum(row[1] for row in pressure_baseline["data"]) != YELLOW_ROWS:
                raise AssertionError("Pressure baseline group counts do not reconcile to the known yellow row count")
            pressure_hash = pressure_baseline["result_sha256"]
            samples: list[dict[str, Any]] = []
            sampling = threading.Event()

            def sample_resources() -> None:
                while not sampling.is_set():
                    try:
                        samples.append({"at_utc": datetime.now(timezone.utc).isoformat(), "pods": top_sample(kube)})
                    except subprocess.SubprocessError as error:
                        samples.append({"at_utc": datetime.now(timezone.utc).isoformat(), "error": str(error)})
                    sampling.wait(0.25)

            sampler = threading.Thread(target=sample_resources, daemon=True)
            sampler.start()
            pressure_results: list[dict[str, Any]] = []
            try:
                for round_index in range(args.pressure_rounds):
                    cases = [dict(pressure_case, name=f"yellow_group_pressure_{index + 1}") for index in range(args.concurrency)]
                    with ThreadPoolExecutor(max_workers=args.concurrency) as executor:
                        futures = [executor.submit(engine.submit, case, include_data=False) for case in cases]
                        for future in as_completed(futures):
                            result = future.result()
                            result["round"] = round_index + 1
                            result["exact"] = result["result_sha256"] == pressure_hash
                            pressure_results.append(result)
            finally:
                sampling.set()
                sampler.join(timeout=35)
            report["pressure"] = {
                "concurrency": args.concurrency,
                "rounds": args.pressure_rounds,
                "requests": pressure_results,
                "baseline": {key: value for key, value in pressure_baseline.items() if key != "data"},
                "baseline_group_count": pressure_baseline["row_count"],
                "known_input_rows": YELLOW_ROWS,
                "resource_samples": samples,
            }

        after_pods = pod_inventory(kube)
        report["pods_after"] = after_pods
        report["retained_files_after"] = {
            "coordinator_exchange": kube.file_count("kaveon-coordinator-0", "/state/exchange"),
            **{name + "_spill": kube.file_count(name, "/tmp/spill") for name in workers},
        }
        unexpected_restarts = {
            name: after_pods[name]["restart_count"] - before_pods[name]["restart_count"]
            for name in before_pods.keys() & after_pods.keys()
            if name != report["worker_loss"]["target"] and after_pods[name]["restart_count"] != before_pods[name]["restart_count"]
        }
        peaks = pressure_peaks(report["pressure"]["resource_samples"])
        engine_pods = ["kaveon-coordinator-0", *workers]
        memory_within_limits = True
        for name in engine_pods:
            limit = (after_pods.get(name, {}).get("resources", {}).get("limits", {}) or {}).get("memory")
            if name not in peaks or not limit or peaks[name]["memory_bytes"] > parse_memory(limit):
                memory_within_limits = False
        report["pressure"]["peak_resources"] = peaks
        report["pressure"]["sampled_engine_memory_within_pod_limits"] = memory_within_limits
        retained_did_not_grow = all(
            after is None or report["retained_files_before"].get(name) is None or after <= report["retained_files_before"][name]
            for name, after in report["retained_files_after"].items()
        )
        pressure_ok = (
            len(report["pressure"]["requests"]) == args.concurrency * args.pressure_rounds
            and all(item["exact"] for item in report["pressure"]["requests"])
            and not unexpected_restarts
            and retained_did_not_grow
            and memory_within_limits
        )
        report["unexpected_restart_deltas"] = unexpected_restarts
        report["checks"]["bounded_pressure_exact_no_unexpected_restart_or_file_growth"] = pressure_ok
        report["checks"]["no_preexisting_retained_exchange_or_spill_files"] = all(
            value == 0 for value in report["retained_files_before"].values() if value is not None
        )
        report["checks"]["post_recovery_exact_probe"] = report["recovery_probe"]["exact"]
        report["passed"] = all(report["checks"].values())
    except Exception as error:
        report["failure"] = str(error)
    finally:
        if port_forward is not None and port_forward.poll() is None:
            port_forward.terminate()
            try:
                port_forward.wait(timeout=10)
            except subprocess.TimeoutExpired:
                port_forward.kill()
                port_forward.wait(timeout=10)
        if port_log is not None:
            port_log.close()
        report["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: AKS fault/pressure report: {args.output}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
