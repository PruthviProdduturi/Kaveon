"""Bounded native Engine operational soak; every completed query is checked against DuckDB.

This is recovery/retention evidence, not a performance comparison. Runtime credentials
and fixtures are ephemeral. Reports include all query outcomes and sampled resources.
"""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import statistics
import subprocess
import tempfile
import threading
import time

import duckdb
import psutil
import pyarrow as pa
import pyarrow.parquet as pq
import requests

from pressure import canonical, stop
from smoke import free_port, wait_for

SQL = {
    "small": "SELECT COUNT(*) AS n, SUM(id) AS total FROM tiny",
    "scan": "SELECT COUNT(*) AS n, SUM(amount) AS total FROM facts WHERE category < 17",
    "group": "SELECT category, COUNT(*) AS n, SUM(amount) AS total FROM facts GROUP BY category",
    "join": "SELECT d.bucket, COUNT(*) AS n, SUM(f.amount) AS total FROM facts f JOIN dimensions d ON f.customer_id = d.id GROUP BY d.bucket",
    "pages": "SELECT id, amount FROM facts ORDER BY id LIMIT 3500",
}
CANCEL_SQL = "SELECT SUM(id) OVER (ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running FROM cancel_values"


def digest(rows):
    return hashlib.sha256("\n".join(canonical(rows)).encode()).hexdigest()


def files(root):
    values = []
    for directory, _, names in os.walk(root):
        for name in names:
            path = Path(directory) / name
            if path.suffix in (".arrow", ".chunk", ".json"):
                with contextlib.suppress(OSError):
                    values.append((str(path.relative_to(root)), path.stat().st_size))
    return values


class Cluster:
    def __init__(self, stack, binary, work, output, workers, rows):
        self.work, self.output = work, output
        self.base = "http://127.0.0.1:" + str(free_port())
        self.tokens = {name: secrets.token_urlsafe(32) for name in ("alice", "bob", "observer")}
        exchange = secrets.token_urlsafe(32)
        principals = [{"principal": name, "token": token, "role": "admin" if name == "observer" else "analyst"} for name, token in self.tokens.items()]
        security = {"principals": principals, "resource_groups": [{"name": "soak", "principals": ["alice", "bob"], "max_running": 1, "max_queued": 8, "queue_timeout_ms": 30000}]}
        self.processes = []
        self.killed = None
        self.started = time.monotonic()
        self.samples = []
        self.done = threading.Event()
        self.sample_errors = []
        for name in ("io", "spill", "exchange", "catalogs"):
            (work / name).mkdir()
        for index in range(workers + 1):
            port = int(self.base.rsplit(":", 1)[1]) if index == 0 else free_port()
            env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
            env.update({"KAVEON_NODE_ID": f"soak-{index}", "KAVEON_ENVIRONMENT": "qualification", "KAVEON_COORDINATOR": str(index == 0).lower(),
                "KAVEON_HTTP_PORT": str(port), "KAVEON_BIND_HOST": "127.0.0.1", "KAVEON_DISCOVERY_URI": self.base,
                "KAVEON_ADVERTISED_URI": f"http://127.0.0.1:{port}", "KAVEON_DATA_DIR": str(work / "data"),
                "KAVEON_CATALOG_DIR": str(work / "catalogs"), "KAVEON_CATALOG_DATABASE_PATH": str(work / f"catalog-{index}.db"),
                "KAVEON_EXCHANGE_TOKEN": exchange, "KAVEON_SECURITY_JSON": json.dumps(security),
                "KAVEON_QUERY_MEMORY_LIMIT_BYTES": str(512 * 1024**2), "KAVEON_MEMORY_ADMISSION_LIMIT_BYTES": str(2 * 1024**3),
                "KAVEON_HASH_SPILL_ROOT": str(work / "spill"), "KAVEON_HASH_SPILL_BYTES": str(1024**3),
                "KAVEON_EXCHANGE_SPOOL_ROOT": str(work / "exchange"), "KAVEON_EXCHANGE_DISK_LIMIT_BYTES": str(2 * 1024**3),
                "TEMP": str(work / "io"), "TMP": str(work / "io"), "TMPDIR": str(work / "io")})
            log = stack.enter_context((output / f"node-{index}.log").open("w", encoding="utf-8"))
            process = subprocess.Popen([str(binary), str(work / "absent.toml")], cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            self.processes.append(process)
            stack.callback(stop, process)
            wait_for(lambda: requests.get(f"http://127.0.0.1:{port}/health", timeout=2).ok, self.processes, timeout=60)
        wait_for(lambda: requests.get(self.base + "/v1/cluster", headers=self.headers("observer"), timeout=2).json()["active_workers"] == workers, self.processes, timeout=60)
        self.thread = threading.Thread(target=self.monitor, daemon=True)
        self.thread.start()
        stack.callback(self.close)

    def headers(self, principal):
        return {"Authorization": "Bearer " + self.tokens[principal]}

    def monitor(self):
        while not self.done.is_set():
            sample = {"elapsed_s": round(time.monotonic() - self.started, 3), "rss_bytes": {}}
            for process in self.processes:
                with contextlib.suppress(psutil.Error):
                    sample["rss_bytes"][str(process.pid)] = psutil.Process(process.pid).memory_info().rss
            retained = files(self.work)
            sample.update(retained_files=len(retained), retained_bytes=sum(size for _, size in retained))
            try:
                response = requests.get(self.base + "/v1/query", headers=self.headers("observer"), timeout=3)
                response.raise_for_status()
                records = response.json()
                if isinstance(records, dict):
                    records = records.get("queries", [])
                sample["history_records"] = len(records)
                sample["running_records"] = sum(record.get("state") == "RUNNING" for record in records)
            except Exception as error:
                self.sample_errors.append({"elapsed_s": sample["elapsed_s"], "error": str(error)})
            self.samples.append(sample)
            self.done.wait(1)

    def close(self):
        self.done.set()
        self.thread.join(timeout=5)

    def submit(self, name, principal, expected):
        started = time.monotonic()
        outcome = {"name": name, "principal": principal, "started_s": round(started-self.started, 3), "passed": False}
        query_id = None
        try:
            response = requests.post(self.base + "/v1/statement", headers=self.headers(principal), json={"query": SQL[name], "result_delivery": "paged", "user": "forged-observer"}, timeout=90)
            payload = response.json()
            outcome["status"] = response.status_code
            query_id = payload.get("id")
            outcome["query_id"] = query_id
            if response.status_code != 200 or payload.get("error") or payload.get("state") != "FINISHED":
                raise AssertionError(str(payload)[:1200])
            rows = payload.get("data") or []
            next_uri = payload.get("next_uri")
            page_count = 0
            while next_uri:
                if not next_uri.startswith(f"/v1/query/{query_id}/results/") or page_count > 10000:
                    raise AssertionError("Unexpected result page path/loop")
                page = requests.get(self.base + next_uri, headers=self.headers(principal), timeout=15)
                page.raise_for_status()
                body = page.json()
                rows.extend(body["data"])
                next_uri = body.get("next_uri")
                page_count += 1
            actual = digest(rows)
            outcome.update(rows=len(rows), result_sha256=actual, expected_sha256=expected[name], result_pages=page_count)
            if actual != expected[name]:
                raise AssertionError("Result checksum mismatch")
            other = "bob" if principal == "alice" else "alice"
            if requests.get(self.base + "/v1/query/" + query_id, headers=self.headers(other), timeout=5).status_code != 404:
                raise AssertionError("Cross-principal history visible")
            record = requests.get(self.base + "/v1/query/" + query_id, headers=self.headers(principal), timeout=5).json()
            if record.get("context", {}).get("user") != principal:
                raise AssertionError("Untrusted request user changed query attribution")
            outcome["passed"] = True
        except Exception as error:
            outcome["failure"] = str(error)
        finally:
            if query_id:
                try:
                    cleaned = requests.delete(self.base + "/v1/query/" + query_id, headers=self.headers(principal), timeout=5)
                    if not cleaned.ok:
                        raise AssertionError("Result cleanup rejected")
                    if requests.get(self.base + f"/v1/query/{query_id}/results/0", headers=self.headers(principal), timeout=5).status_code != 404:
                        raise AssertionError("Deleted query result page remains readable")
                except Exception as error:
                    outcome.update(passed=False, cleanup_error=str(error))
        outcome["latency_ms"] = round((time.monotonic()-started)*1000, 3)
        return outcome

    def cancel_and_queue(self, executor, expected):
        outcome = {"name": "active_cancel_and_queue", "passed": False}
        started = time.monotonic()
        future = executor.submit(requests.post, self.base + "/v1/statement", headers=self.headers("alice"), json={"query": CANCEL_SQL, "result_delivery": "paged"}, timeout=60)
        try:
            query_id = None
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not future.done():
                records = requests.get(self.base + "/v1/query", headers=self.headers("alice"), timeout=3).json()
                if isinstance(records, dict):
                    records = records.get("queries", [])
                active = [r for r in records if r.get("sql") == CANCEL_SQL and r.get("state") == "RUNNING"]
                if active:
                    query_id = active[0]["id"]
                    break
                time.sleep(0.025)
            if not query_id:
                raise AssertionError("No running cancellation query observed")
            queued = executor.submit(self.submit, "small", "bob", expected)
            time.sleep(0.25)
            if future.done() or queued.done():
                raise AssertionError("Cancellation CPU workload or queued request finished before checkpoint")
            cancel_started = time.monotonic()
            deleted = requests.delete(self.base + "/v1/query/" + query_id, headers=self.headers("alice"), timeout=5)
            reply = future.result(timeout=10)
            probe = queued.result(timeout=10)
            record = requests.get(self.base + "/v1/query/" + query_id, headers=self.headers("alice"), timeout=5).json()
            outcome.update(cancel_status=deleted.status_code, statement_status=reply.status_code, state=record.get("state"), queued_probe=probe, recovery_ms=round((time.monotonic()-cancel_started)*1000, 3))
            if not deleted.ok or reply.status_code != 409 or record.get("state") != "CANCELED" or not probe["passed"]:
                raise AssertionError("Cancellation or queued admission recovery failed")
            outcome["passed"] = True
        except Exception as error:
            outcome["failure"] = str(error)
        outcome["latency_ms"] = round((time.monotonic()-started)*1000, 3)
        return outcome


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--duration-seconds", type=int, default=120)
    parser.add_argument("--workers", type=int, choices=[2, 5], default=2)
    parser.add_argument("--rows", type=int, default=100000)
    parser.add_argument("--max-rss-growth-mib", type=int, default=256)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.server_bin.resolve()
    report = {"started_at": datetime.now(timezone.utc).isoformat(), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "workers": args.workers, "rows": args.rows, "requested_duration_seconds": args.duration_seconds, "queries": [], "controls": [], "passed": False}
    for label, command in [("git_revision", ["git", "rev-parse", "HEAD"]), ("source_diff_sha256", ["git", "diff", "--binary"])]:
        result = subprocess.run(command, capture_output=True, check=True)
        report[label] = hashlib.sha256(result.stdout).hexdigest() if label.endswith("sha256") else result.stdout.decode().strip()
    source_paths = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard", "engine/crates", "api", "engine/Cargo.toml", "engine/Cargo.lock", "engine/qualification/soak.py"], capture_output=True, check=True).stdout.decode().splitlines()
    source_hash = hashlib.sha256()
    for source_path in sorted(set(source_paths)):
        path = Path(source_path)
        if path.is_file() and path.suffix in (".rs", ".py", ".toml", ".lock"):
            source_hash.update(source_path.encode() + b"\0" + path.read_bytes())
    report["source_tree_sha256"] = source_hash.hexdigest()
    try:
        with contextlib.ExitStack() as stack:
            work = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-soak-")))
            (work / "data").mkdir()
            isolated_binary = work / binary.name
            shutil.copy2(binary, isolated_binary)
            if hashlib.sha256(isolated_binary.read_bytes()).hexdigest() != report["binary_sha256"]:
                raise AssertionError("Server binary changed while copying; retry with stable build")
            db = duckdb.connect()
            stack.callback(db.close)
            fixtures = {"facts": pa.table({"id": range(args.rows), "customer_id": [i%1000 for i in range(args.rows)], "category": [i%100 for i in range(args.rows)], "amount": [i%997 for i in range(args.rows)]}),
                        "dimensions": pa.table({"id": range(1000), "bucket": [i%10 for i in range(1000)]}), "tiny": pa.table({"id": range(1000)}), "cancel_values": pa.table({"id": range(30000)})}
            report["fixtures"] = {}
            for name, table in fixtures.items():
                path = work / "data" / (name + ".parquet")
                pq.write_table(table, path, row_group_size=8192)
                report["fixtures"][name] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "rows": table.num_rows}
                db.execute(f"CREATE VIEW {name} AS SELECT * FROM read_parquet('{path.as_posix()}')")
            expected = {name: digest(db.execute(sql).fetchall()) for name, sql in SQL.items()}
            report["sql"] = SQL
            cluster = Cluster(stack, isolated_binary, work, args.output, args.workers, args.rows)
            with ThreadPoolExecutor(max_workers=8) as executor:
                started = time.monotonic()
                deadline = started + args.duration_seconds
                next_cancel = started
                killed = False
                cycle = 0
                while time.monotonic() < deadline:
                    if time.monotonic() >= next_cancel:
                        report["controls"].append(cluster.cancel_and_queue(executor, expected))
                        next_cancel = time.monotonic() + 25
                    tasks = [executor.submit(cluster.submit, name, "alice" if (cycle+i)%2 == 0 else "bob", expected) for i, name in enumerate(SQL)]
                    if not killed and time.monotonic() >= started + args.duration_seconds * 0.5:
                        stop(cluster.processes[1])
                        cluster.killed = cluster.processes[1].pid
                        report["worker_loss"] = {"pid": cluster.killed, "elapsed_s": round(time.monotonic()-started, 3)}
                        killed = True
                    outcomes = [task.result(timeout=100) for task in tasks]
                    report["queries"].extend(outcomes)
                    cycle += 1
                    if cycle % 10 == 0:
                        progress = {"elapsed_s": round(time.monotonic()-started, 3), "queries": len(report["queries"]), "controls": len(report["controls"]), "failures": sum(not q["passed"] for q in report["queries"]), "worker_killed": killed}
                        (args.output / "live-status.json").write_text(json.dumps(progress), encoding="utf-8")
                        print(json.dumps(progress), flush=True)
                    if any(not item["passed"] for item in outcomes) or any(not item["passed"] for item in report["controls"]):
                        break
                    time.sleep(0.2)
                report["actual_workload_seconds"] = round(time.monotonic()-started, 3)
                # Recovery probe must succeed after any intentional worker loss.
                report["queries"].append(cluster.submit("join", "alice", expected))
                settle_deadline = time.monotonic()+10
                while files(work) and time.monotonic() < settle_deadline:
                    time.sleep(0.25)
                cluster.close()
                report["samples"] = cluster.samples
                report["sample_errors"] = cluster.sample_errors
                report["remaining_retained_files"] = files(work)
                report["max_history_records"] = max((s.get("history_records", 0) for s in cluster.samples), default=0)
                report["rss"] = {}
                for process in cluster.processes:
                    values = [(s["elapsed_s"], s["rss_bytes"][str(process.pid)]) for s in cluster.samples if str(process.pid) in s["rss_bytes"]]
                    if not values:
                        continue
                    warm = [value for elapsed, value in values if 10 <= elapsed <= 30] or [value for _, value in values[:5]]
                    tail = [value for _, value in values[-10:]]
                    growth = statistics.median(tail)-statistics.median(warm)
                    report["rss"][str(process.pid)] = {"peak_bytes": max(value for _, value in values), "warm_median_bytes": statistics.median(warm), "tail_median_bytes": statistics.median(tail), "growth_bytes": growth, "intentionally_killed": process.pid == cluster.killed}
                report["checks"] = {
                    "all_query_results": all(q["passed"] for q in report["queries"]), "cancel_and_queue": len(report["controls"]) >= 2 and all(q["passed"] for q in report["controls"]),
                    "worker_loss_exercised": killed, "duration_reached": report["actual_workload_seconds"] >= args.duration_seconds,
                    "history_bounded": report["max_history_records"] <= 110, "retained_files_cleaned": not report["remaining_retained_files"],
                    "monitor_responsive": not report["sample_errors"], "rss_growth_bounded": all(r["growth_bytes"] <= args.max_rss_growth_mib*1024**2 for r in report["rss"].values()),
                    "rss_peak_bounded": all(r["peak_bytes"] <= 2*1024**3 for r in report["rss"].values()),
                    "surviving_nodes_alive": all(p.poll() is None for p in cluster.processes if p.pid != cluster.killed)}
                report["passed"] = all(report["checks"].values())
    except Exception as error:
        report["fatal_error"] = str(error)
    latencies = [q["latency_ms"] for q in report["queries"] if q.get("passed")]
    report["latency_ms"] = {"median": statistics.median(latencies) if latencies else None, "p95": sorted(latencies)[min(len(latencies)-1, int(len(latencies)*.95))] if latencies else None}
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps({key: report.get(key) for key in ("passed", "checks", "fatal_error", "actual_workload_seconds", "latency_ms", "rss")}, indent=2))
    print(f"Queries: {len(report['queries'])}; controls: {len(report['controls'])}; report: {args.output / 'report.json'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
