"""Kill a consumer worker only after upstream exchanges are complete.

A local forwarding proxy provides a deterministic barrier at the join stage;
the Engine receives no test-only execution hooks. Run with qualification/venv.
"""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time

import duckdb
import pyarrow.parquet as pq
import requests


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def proxy_handler(target, reached, released, intercept):
    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            if intercept and self.path == "/v1/task" and json.loads(body).get("stage_id") == 2:
                reached.set()
                if not released.wait(15):
                    self.send_error(504)
                    return
            try:
                response = requests.post(target + self.path, data=body,
                                         headers={key: value for key, value in self.headers.items() if key.lower() not in {"host", "content-length"}}, timeout=120)
                self.send_response(response.status_code)
                for key in ("Content-Type", "x-kaveon-task-elapsed-us"):
                    if key in response.headers:
                        self.send_header(key, response.headers[key])
                self.send_header("Content-Length", str(len(response.content)))
                self.end_headers()
                self.wfile.write(response.content)
            except requests.RequestException:
                self.send_error(502, "upstream worker unavailable")
    return Proxy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rows", type=int, default=1_000_000)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.server_bin.resolve()
    report = {"rows": args.rows, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "passed": False}
    with contextlib.ExitStack() as stack:
        work = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-exchange-loss-")))
        data = work / "data"
        data.mkdir()
        (work / "catalogs").mkdir()
        spool = work / "exchanges"
        spool.mkdir()
        db = stack.enter_context(duckdb.connect())
        events = db.execute(f"SELECT i::BIGINT AS event_id, (i%10000)::BIGINT AS customer_id, (i%1000)::BIGINT AS amount FROM range({args.rows}) t(i)").to_arrow_table()
        customers = db.execute("SELECT i::BIGINT AS customer_id FROM range(10000) t(i)").to_arrow_table()
        pq.write_table(events, data / "events.parquet", row_group_size=50000)
        pq.write_table(customers, data / "customers.parquet", row_group_size=1000)
        db.register("events", events)
        db.register("customers", customers)
        sql = "SELECT COUNT(*), SUM(e.amount) FROM events e JOIN customers c ON e.customer_id=c.customer_id"
        expected = [list(row) for row in db.execute(sql).fetchall()]
        coordinator_port = port()
        base = f"http://127.0.0.1:{coordinator_port}"
        token, exchange_token = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        headers = {"Authorization": "Bearer " + token}
        reached, released = threading.Event(), threading.Event()
        processes = []
        for index in range(3):
            actual_port = coordinator_port if index == 0 else port()
            advertised = f"http://127.0.0.1:{actual_port}"
            if index:
                proxy = ThreadingHTTPServer(("127.0.0.1", 0), proxy_handler(advertised, reached, released, index == 1))
                advertised = f"http://127.0.0.1:{proxy.server_address[1]}"
                threading.Thread(target=proxy.serve_forever, daemon=True).start()
                stack.callback(proxy.server_close)
                stack.callback(proxy.shutdown)
            env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
            env.update({"KAVEON_NODE_ID": f"exchange-loss-{index}", "KAVEON_COORDINATOR": str(index == 0).lower(),
                        "KAVEON_HTTP_PORT": str(actual_port), "KAVEON_ADVERTISED_URI": advertised,
                        "KAVEON_DISCOVERY_URI": base, "KAVEON_DATA_DIR": str(data), "KAVEON_CATALOG_DIR": str(work / "catalogs"),
                        "KAVEON_CATALOG_DATABASE_PATH": str(work / f"catalog-{index}.db"),
                        "KAVEON_EXCHANGE_SPOOL_ROOT": str(spool), "KAVEON_EXCHANGE_TOKEN": exchange_token,
                        "KAVEON_SECURITY_JSON": json.dumps({"principals": [{"token": token, "principal": "qualification", "role": "analyst"}]})})
            output = stack.enter_context((args.output / f"node-{index}.log").open("w"))
            process = subprocess.Popen([str(binary), str(work / "absent.toml")], cwd=work, env=env, stdout=output, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            processes.append(process)
            stack.callback(stop, process)
            deadline = time.monotonic() + 20
            while True:
                try:
                    if requests.get(f"http://127.0.0.1:{actual_port}/health", timeout=1).ok:
                        break
                except requests.RequestException:
                    pass
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("Engine failed to start")
                time.sleep(0.1)
        deadline = time.monotonic() + 20
        while requests.get(base + "/v1/cluster", headers=headers, timeout=2).json()["active_workers"] != 2:
            if time.monotonic() > deadline:
                raise RuntimeError("Workers did not register")
            time.sleep(0.1)
        with ThreadPoolExecutor(max_workers=1) as executor:
            query = executor.submit(requests.post, base + "/v1/statement", headers=headers, json={"query": sql}, timeout=120)
            try:
                if not reached.wait(30):
                    raise AssertionError("Join consumer barrier was not reached")
                report["producer_chunks_before_failure"] = len(list(spool.rglob("*.chunk")))
                if not report["producer_chunks_before_failure"]:
                    raise AssertionError("No coordinator exchanges were materialized")
                stop(processes[1])
                released.set()
                response = query.result(timeout=90)
                result = response.json()
                report.update(status=response.status_code, data=result.get("data"), error=result.get("error"), expected=expected)
                report["exchange_files_after_completion"] = len(list(spool.rglob("*.chunk")))
                report["passed"] = response.ok and result.get("data") == expected and report["exchange_files_after_completion"] == 0
            finally:
                released.set()
    (args.output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
