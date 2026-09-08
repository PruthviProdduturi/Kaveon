"""Local Engine pressure qualification: pool failures, spill, RSS, and cancellation.

RSS is observed independently; it is never described as the logical pool limit.
This harness does not require Trino. DuckDB reads the same Parquet fixtures.
"""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import threading
import time

import duckdb
import psutil
import pyarrow as pa
import pyarrow.parquet as pq
import requests

from smoke import free_port, wait_for


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def arrow_files(root):
    # os.walk tolerates directories removed concurrently by spill RAII cleanup.
    for directory, _, files in os.walk(root):
        for name in files:
            if name.endswith(".arrow"):
                yield Path(directory) / name


class Monitor:
    """Sample native server RSS and currently present Arrow spill files."""
    def __init__(self, processes, spill_root):
        self.processes = [psutil.Process(process.pid) for process in processes]
        self.spill_root = spill_root
        self.lock = threading.Lock()
        self.done = threading.Event()
        self.active = None
        self.results = {}
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def begin(self, name):
        with self.lock:
            self.active = name
            self.results[name] = {"samples": 0, "peak_rss_bytes_by_pid": {},
                                  "peak_spill_bytes": 0, "peak_spill_files": 0}

    def sample(self):
        rss = {}
        for process in self.processes:
            with contextlib.suppress(psutil.Error):
                rss[str(process.pid)] = process.memory_info().rss
        sizes = []
        for path in arrow_files(self.spill_root):
            with contextlib.suppress(OSError):
                sizes.append(path.stat().st_size)
        with self.lock:
            if self.active:
                result = self.results[self.active]
                result["samples"] += 1
                for pid, size in rss.items():
                    result["peak_rss_bytes_by_pid"][pid] = max(size, result["peak_rss_bytes_by_pid"].get(pid, 0))
                result["peak_spill_bytes"] = max(result["peak_spill_bytes"], sum(sizes))
                result["peak_spill_files"] = max(result["peak_spill_files"], len(sizes))

    def run(self):
        while not self.done.wait(0.025):
            self.sample()

    def finish(self, name):
        self.sample()
        with self.lock:
            return json.loads(json.dumps(self.results[name]))

    def close(self):
        self.done.set()
        self.thread.join(timeout=2)


class Engine:
    def __init__(self, stack, binary, data, output, memory_bytes, disk_bytes, workers, profile):
        self.memory_bytes = memory_bytes
        self.disk_bytes = disk_bytes
        self.work = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-pressure-")))
        (self.work / "catalogs").mkdir()
        self.spill = self.work / "spill"
        self.spill.mkdir()
        port = free_port()
        self.base = f"http://127.0.0.1:{port}"
        token, exchange_token = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        self.headers = {"Authorization": "Bearer " + token}
        self.processes = []
        for index in range(workers + 1):
            node_port = port if index == 0 else free_port()
            env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
            env.update({
                "KAVEON_NODE_ID": f"pressure-{profile}-{index}",
                "KAVEON_ENVIRONMENT": "qualification",
                "KAVEON_COORDINATOR": str(index == 0).lower(),
                "KAVEON_HTTP_PORT": str(node_port),
                "KAVEON_BIND_HOST": "127.0.0.1",
                "KAVEON_DISCOVERY_URI": self.base,
                "KAVEON_ADVERTISED_URI": f"http://127.0.0.1:{node_port}",
                "KAVEON_DATA_DIR": str(data),
                "KAVEON_CATALOG_DIR": str(self.work / "catalogs"),
                "KAVEON_CATALOG_DATABASE_PATH": str(self.work / f"catalog-{index}.db"),
                "KAVEON_EXCHANGE_TOKEN": exchange_token,
                "KAVEON_QUERY_MEMORY_LIMIT_BYTES": str(memory_bytes),
                "KAVEON_MEMORY_ADMISSION_LIMIT_BYTES": str(memory_bytes),
                "KAVEON_HASH_SPILL_ROOT": str(self.spill),
                "KAVEON_HASH_SPILL_BYTES": str(disk_bytes),
                "KAVEON_HASH_SPILL_PARTITIONS": "16",
                "KAVEON_SECURITY_JSON": json.dumps({"principals": [{
                    "token": token, "principal": "pressure", "role": "analyst"}]}),
            })
            log = stack.enter_context((output / f"{profile}-node-{index}.log").open("w", encoding="utf-8"))
            process = subprocess.Popen([str(binary), str(self.work / "absent.toml")], cwd=self.work,
                                       env=env, stdout=log, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            self.processes.append(process)
            stack.callback(stop, process)
            wait_for(lambda: requests.get(f"http://127.0.0.1:{node_port}/health", timeout=2).ok,
                     self.processes, timeout=60)
        wait_for(lambda: requests.get(self.base + "/v1/cluster", headers=self.headers, timeout=2).json()["active_workers"] == workers,
                 self.processes, timeout=60)
        self.monitor = Monitor(self.processes, self.spill)
        stack.callback(self.monitor.close)

    def submit(self, sql, timeout=90):
        response = requests.post(self.base + "/v1/statement", headers=self.headers,
                                 json={"query": sql, "result_delivery": "paged"}, timeout=timeout)
        result = response.json()
        rows = result.get("data") or []
        next_uri = result.get("next_uri")
        while next_uri:
            page = requests.get(self.base + next_uri, headers=self.headers, timeout=30)
            page.raise_for_status()
            payload = page.json()
            rows.extend(payload["data"])
            next_uri = payload.get("next_uri")
        return response.status_code, result, rows

    def cleanup(self, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not list(arrow_files(self.spill)):
                return True
            time.sleep(0.05)
        return False


def canonical(rows):
    def scalar(value):
        if isinstance(value, float) and value.is_integer():
            return int(value)
        return value
    return sorted(json.dumps([scalar(value) for value in row], separators=(",", ":"), default=str) for row in rows)


def run_case(engine, db, name, sql, expect_error=False, require_spill=False):
    case = {"name": name, "sql": sql, "expect_budget_rejection": expect_error, "passed": False,
            "configured_query_pool_bytes": engine.memory_bytes, "configured_spill_pool_bytes": engine.disk_bytes,
            "requires_observed_spill": require_spill}
    engine.monitor.begin(name)
    started = time.monotonic()
    try:
        status, result, rows = engine.submit(sql)
        case.update(status=status, query_id=result.get("id"), state=result.get("state"))
        error = result.get("error")
        if expect_error:
            message = json.dumps(error) if error else ""
            case["error"] = error
            if not any(marker in message.lower() for marker in ("cannot reserve", "spill limit", "string expression output exceeds")):
                raise AssertionError("Expected a specific memory/disk/expansion rejection; got " + str(result)[:1500])
        else:
            if status != 200 or error or result.get("state") != "FINISHED":
                raise AssertionError(str(result)[:1500])
            expected = canonical(db.execute(sql).fetchall())
            actual = canonical(rows)
            if actual != expected:
                raise AssertionError(f"Result mismatch: actual rows {len(actual)}, expected {len(expected)}")
            case.update(rows=len(actual), result_sha256=hashlib.sha256("\n".join(actual).encode()).hexdigest())
        case["spill_files_cleaned"] = engine.cleanup()
        if not case["spill_files_cleaned"]:
            raise AssertionError("Spill files remain after query completion/error")
        health = requests.get(engine.base + "/health", timeout=3)
        if not health.ok:
            raise AssertionError("Server health failed after pressure query")
        case["passed"] = True
    except Exception as error:
        case["failure"] = str(error)
    case["elapsed_ms"] = round((time.monotonic() - started) * 1000, 2)
    case["observed"] = engine.monitor.finish(name)
    if require_spill:
        case["spill_observed"] = case["observed"]["peak_spill_files"] > 0
        if not case["spill_observed"]:
            case.update(passed=False, failure="No sampled evidence that the required spill path ran")
    print(f"{'PASS' if case['passed'] else 'FAIL'} {name}: {case.get('failure', '')}", flush=True)
    return case


def cancellation_case(engine):
    name = "cancel_running_window"
    sql = "SELECT SUM(id) OVER (ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running FROM cancel_values"
    case = {"name": name, "sql": sql, "passed": False,
            "configured_query_pool_bytes": engine.memory_bytes, "configured_spill_pool_bytes": engine.disk_bytes}
    engine.monitor.begin(name)
    started = time.monotonic()
    executor = ThreadPoolExecutor(max_workers=1)
    future = executor.submit(engine.submit, sql, 60)
    try:
        query_id = None
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and not future.done():
            response = requests.get(engine.base + "/v1/query", headers=engine.headers, timeout=2)
            response.raise_for_status()
            records = response.json()
            if isinstance(records, dict):
                records = records.get("queries", [])
            matching = [record for record in records if record.get("sql") == sql and record.get("state") in ("RUNNING", "QUEUED", "PLANNING")]
            if matching:
                query_id = matching[0].get("id") or matching[0].get("query_id")
                break
            time.sleep(0.025)
        if not query_id:
            raise AssertionError("Could not observe an active window query before completion")
        # Let the synchronous operator enter its frame loop; canceling before
        # planning finishes does not qualify computational cancellation.
        time.sleep(0.25)
        if future.done():
            raise AssertionError("Window completed before the computational cancellation checkpoint")
        cancel_started = time.monotonic()
        case["running_before_cancel_ms"] = round((cancel_started - started) * 1000, 2)
        canceled = requests.delete(engine.base + "/v1/query/" + query_id, headers=engine.headers, timeout=5)
        case.update(query_id=query_id, cancel_status=canceled.status_code)
        if not canceled.ok:
            raise AssertionError("Cancellation request failed: " + canceled.text[:500])
        status, result, _ = future.result(timeout=5)
        case.update(statement_status=status, terminal_state=result.get("state"), error=result.get("error"))
        record = requests.get(engine.base + "/v1/query/" + query_id, headers=engine.headers, timeout=3).json()
        if record.get("state") != "CANCELED":
            raise AssertionError("Query history did not retain CANCELED: " + str(record)[:500])
        case["spill_files_cleaned"] = engine.cleanup()
        if not case["spill_files_cleaned"]:
            raise AssertionError("Canceled query left spill files")
        deadline = cancel_started + 5
        while time.monotonic() < deadline:
            status, result, rows = engine.submit("SELECT COUNT(*) FROM tiny", timeout=3)
            if status == 200 and not result.get("error") and rows == [[1000]]:
                break
            if status != 429:
                raise AssertionError("Post-cancellation probe failed: " + str(result)[:500])
            time.sleep(0.025)
        else:
            raise AssertionError("Admission did not recover within five seconds of cancellation")
        case["cancel_to_admission_recovery_ms"] = round((time.monotonic() - cancel_started) * 1000, 2)
        case["passed"] = True
    except Exception as error:
        case["failure"] = str(error)
        # A stuck CPU-bound query must not keep the harness alive indefinitely.
        for process in engine.processes:
            stop(process)
    finally:
        executor.shutdown(wait=True, cancel_futures=True)
    case["elapsed_ms"] = round((time.monotonic() - started) * 1000, 2)
    case["observed"] = engine.monitor.finish(name)
    print(f"{'PASS' if case['passed'] else 'FAIL'} {name}: {case.get('failure', '')}", flush=True)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rows", type=int, default=100_000)
    parser.add_argument("--workers", type=int, choices=[0, 2, 5], default=0)
    parser.add_argument("--memory-mib", type=int, default=32)
    parser.add_argument("--skip-cancellation", action="store_true")
    args = parser.parse_args()
    if args.rows < 50_000 or args.memory_mib < 8:
        parser.error("Use at least 50,000 rows and an 8 MiB pool for meaningful pressure fixtures")
    source_binary = args.server_bin.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.output.resolve() / ("engine-under-test" + source_binary.suffix)
    shutil.copy2(source_binary, binary)
    report = {"benchmark": False, "workers": args.workers, "rows": args.rows,
              "timestamp_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
              "configured_query_pool_bytes": args.memory_mib * 1024 * 1024,
              "rss_is_logical_pool_limit": False,
              "rss_note": "25ms sampled native process RSS includes allocator/Arrow/storage/transport/runtime memory outside logical operator estimates; peaks between samples may be missed.",
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "source_binary": str(source_binary),
              "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "working_tree": subprocess.check_output(["git", "status", "--porcelain"], text=True).splitlines(),
              "versions": {"duckdb": duckdb.__version__, "pyarrow": pa.__version__, "psutil": psutil.__version__},
              "cases": []}
    with contextlib.ExitStack() as stack:
        data = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-pressure-data-")))
        db = stack.enter_context(duckdb.connect())
        fixtures = {
            "facts": pa.table({"id": pa.array(range(args.rows), type=pa.int64()), "k": pa.array([index % 1024 for index in range(args.rows)], type=pa.int64())}),
            "dimensions": pa.table({"id": pa.array(range(0, args.rows, 10), type=pa.int64())}),
            "skew": pa.table({"k": pa.array([1] * 5000, type=pa.int64())}),
            "tiny": pa.table({"id": pa.array(range(1000), type=pa.int64())}),
            "cancel_values": pa.table({"id": pa.array(range(30000), type=pa.int64())}),
            "window_pressure": pa.table({"id": pa.array(range(max(args.rows, args.memory_mib * 1024)), type=pa.int64())}),
        }
        report["fixtures"] = {}
        for name, table in fixtures.items():
            path = data / f"{name}.parquet"
            pq.write_table(table, path, row_group_size=4096, compression="snappy")
            db.execute(f"CREATE VIEW {name} AS SELECT * FROM read_parquet('{path.as_posix()}')")
            report["fixtures"][name] = {"rows": table.num_rows, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        del fixtures
        with contextlib.ExitStack() as profile:
            engine = Engine(profile, binary, data, args.output, args.memory_mib * 1024 * 1024, 256 * 1024 * 1024, args.workers, "mixed")
            for name, sql, reject in [
                ("grouped_spill", "SELECT id, COUNT(*) FROM facts GROUP BY id", False),
                ("join", "SELECT COUNT(*) FROM facts f JOIN dimensions d ON f.id = d.id", False),
                ("sort_spill", "SELECT id FROM facts ORDER BY id DESC", False),
                ("topn", "SELECT id FROM facts ORDER BY id DESC LIMIT 20", False),
                ("set_operation", "SELECT id FROM tiny INTERSECT SELECT id FROM dimensions", False),
                ("bounded_window", "SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS n FROM tiny", False),
                ("skew_join_rejection", "SELECT COUNT(*) FROM skew l JOIN skew r ON l.k = r.k", True),
                ("window_rejection", "SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS n FROM window_pressure", True),
                ("repeat_rejection", "SELECT REPEAT('x', 1000000000) FROM tiny LIMIT 1", True),
            ]:
                report["cases"].append(run_case(engine, db, name, sql, reject,
                    require_spill=name == "grouped_spill" or (name == "sort_spill" and args.memory_mib <= 32)))
        with contextlib.ExitStack() as profile:
            # Keep this local quota fixture small enough to require spilling even
            # when the distributed profile uses a larger pool. Adaptive execution
            # may legitimately finish entirely in memory at 256 MiB.
            engine = Engine(profile, binary, data, args.output, min(args.memory_mib, 32) * 1024 * 1024, 1024, 0, "diskquota")
            report["cases"].append(run_case(engine, db, "diskquota_rejection", "SELECT id, COUNT(*) FROM facts GROUP BY id", True))
        if not args.skip_cancellation:
            with contextlib.ExitStack() as profile:
                engine = Engine(profile, binary, data, args.output, 256 * 1024 * 1024, 256 * 1024 * 1024, 0, "cancellation")
                report["cases"].append(cancellation_case(engine))
    report["passed"] = all(case["passed"] for case in report["cases"])
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
