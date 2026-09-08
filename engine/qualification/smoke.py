"""Local semantic qualification; fixture-scale correctness, never a speed benchmark."""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import time

import duckdb
import pyarrow as pa
import pyarrow.parquet as pq
import requests
import trino.dbapi


FIXTURES = {
    "left_values": [(1,), (2,), (None,)],
    "right_values": [(1,), (None,)],
    "measurements": [(1,), (1,), (2,), (4,)],
    "six_values": [(1,), (2,), (3,), (4,), (5,), (6,)],
    "signed_values": [(-3,), (-2,), (-1,), (0,), (1,), (2,), (None,)],
    "large_values": [(9007199254740993,), (-9007199254740992,), (2,), (None,)],
}
BASELINE = {
    "scan": "SELECT x FROM left_values ORDER BY x",
    "filtered_count": "SELECT COUNT(*) FROM left_values WHERE x > 1",
    "grouped_count": "SELECT x, COUNT(*) FROM measurements GROUP BY x ORDER BY x",
    "join_count": "SELECT COUNT(*) FROM left_values l JOIN measurements m ON l.x = m.x",
    "topn": "SELECT x FROM measurements ORDER BY x DESC LIMIT 2",
}
REGRESSIONS = {
    "not_in_null": "SELECT x FROM left_values WHERE x NOT IN (SELECT x FROM right_values) ORDER BY x",
    "uncorrelated_exists": "SELECT x FROM left_values WHERE EXISTS (SELECT x FROM right_values) ORDER BY x",
    "range_peers": "SELECT x, COUNT(*) OVER (ORDER BY x RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS n FROM measurements ORDER BY x",
    "groups_peers": "SELECT x, COUNT(*) OVER (ORDER BY x GROUPS BETWEEN CURRENT ROW AND CURRENT ROW) AS n FROM measurements ORDER BY x",
    "empty_frame": "SELECT x, COUNT(*) OVER (ORDER BY x ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING) AS n FROM measurements ORDER BY x",
    "default_frame": "SELECT x, COUNT(*) OVER (ORDER BY x) AS n FROM measurements ORDER BY x",
    "group_preceding": "SELECT x, COUNT(*) OVER (ORDER BY x GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) AS n FROM measurements ORDER BY x",
    "empty_sum": "SELECT x, SUM(x) OVER (ORDER BY x ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING) AS n FROM measurements ORDER BY x",
    "ranks": "SELECT x, RANK() OVER (ORDER BY x) AS r, DENSE_RANK() OVER (ORDER BY x) AS d FROM measurements ORDER BY x",
    "ntile": "SELECT x, NTILE(4) OVER (ORDER BY x) AS n FROM six_values ORDER BY x",
    "frame_values": "SELECT x, FIRST_VALUE(x) OVER (ORDER BY x ROWS BETWEEN CURRENT ROW AND CURRENT ROW) AS f, LAST_VALUE(x) OVER (ORDER BY x ROWS BETWEEN CURRENT ROW AND CURRENT ROW) AS l FROM measurements ORDER BY x",
    "lag_default": "SELECT x, LAG(x,1,99) OVER (ORDER BY x) AS n FROM six_values ORDER BY x",
    "not_in_empty": "SELECT x FROM left_values WHERE x NOT IN (SELECT x FROM right_values WHERE x > 100) ORDER BY x",
    "not_in_nonempty": "SELECT x FROM left_values WHERE x NOT IN (SELECT x FROM right_values WHERE x IS NOT NULL) ORDER BY x",
    "exists_null": "SELECT x FROM left_values WHERE EXISTS (SELECT x FROM right_values WHERE x IS NULL) ORDER BY x",
    "not_exists_empty": "SELECT x FROM left_values WHERE NOT EXISTS (SELECT x FROM right_values WHERE x > 100) ORDER BY x",
    "in_null_literal": "SELECT x FROM signed_values WHERE x IN (1, NULL) ORDER BY x",
    "not_in_null_literal": "SELECT x FROM signed_values WHERE x NOT IN (1, NULL) ORDER BY x",
    "empty_global_aggregates": "SELECT COUNT(*), COUNT(x), SUM(x), MIN(x), MAX(x), AVG(x) FROM signed_values WHERE x > 100",
    "typed_null_groups": "SELECT CAST(x AS INTEGER) AS k, COUNT(*) FROM signed_values GROUP BY CAST(x AS INTEGER) ORDER BY k",
    "decimal_groups": "SELECT CAST(CAST(x AS DECIMAL(20,4)) AS VARCHAR) AS k, COUNT(*) FROM signed_values GROUP BY CAST(x AS DECIMAL(20,4)) ORDER BY k",
    "decimal_sum": "SELECT CAST(SUM(CAST(x AS DECIMAL(20,4))) AS VARCHAR) FROM signed_values",
    "negative_arithmetic": "SELECT x, x * 2 - 1 FROM signed_values ORDER BY x",
    "null_comparison": "SELECT x FROM signed_values WHERE NOT (x = 1 OR x = 2) ORDER BY x",
    "left_join_nulls": "SELECT l.x, r.x FROM signed_values l LEFT JOIN right_values r ON l.x = r.x ORDER BY l.x, r.x",
    "right_join_nulls": "SELECT l.x, r.x FROM signed_values l RIGHT JOIN right_values r ON l.x = r.x ORDER BY l.x, r.x",
    "full_join_nulls": "SELECT l.x, r.x FROM signed_values l FULL JOIN right_values r ON l.x = r.x ORDER BY l.x, r.x",
    "intersect_dedup": "SELECT x FROM measurements INTERSECT SELECT x FROM left_values ORDER BY x",
    "except_dedup": "SELECT x FROM measurements EXCEPT SELECT x FROM right_values ORDER BY x",
    "union_dedup": "SELECT x FROM measurements UNION SELECT x FROM right_values ORDER BY x",
    "large_integer_aggregates": "SELECT SUM(x), MIN(x), MAX(x) FROM large_values",
    "large_integer_distinct_sum": "SELECT SUM(DISTINCT x) FROM large_values",
}


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(check, processes, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if any(process.poll() is not None for process in processes):
            raise RuntimeError("Engine process exited; inspect the output directory logs")
        try:
            if check():
                return
        except requests.RequestException:
            pass
        time.sleep(0.25)
    raise TimeoutError("Engine readiness/worker discovery timed out")


def reference_sql(sql):
    tables = []
    for name, rows in FIXTURES.items():
        values = ",".join("(CAST(NULL AS BIGINT))" if row[0] is None else f"(CAST({row[0]} AS BIGINT))" for row in rows)
        tables.append(f"{name}(x) AS (VALUES {values})")
    return "WITH " + ",".join(tables) + " " + sql


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--workers", type=int, choices=[0, 2, 5], default=0)
    parser.add_argument("--concurrency", type=int, default=0)
    parser.add_argument("--worker-loss", action="store_true")
    parser.add_argument("--regressions", action="store_true")
    parser.add_argument("--trino-port", type=int, default=18080)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.server_bin.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    frozen_binary = args.output.resolve() / ("qualified-" + binary.name)
    shutil.copy2(binary, frozen_binary)
    binary = frozen_binary
    report = {"workers": args.workers, "benchmark": False, "cases": []}
    report["commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    report["engine_tree"] = subprocess.check_output(["git", "rev-parse", "HEAD:engine"], text=True).strip()
    report["working_tree"] = subprocess.check_output(["git", "status", "--porcelain"], text=True).splitlines()
    report["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    report["reference"] = {"duckdb": duckdb.__version__, "pyarrow": pa.__version__}
    with contextlib.ExitStack() as stack:
        work = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-qualification-")))
        data = work / "data"
        data.mkdir()
        (work / "catalogs").mkdir()
        db = stack.enter_context(duckdb.connect())
        for name, rows in FIXTURES.items():
            table = pa.table({"x": pa.array([row[0] for row in rows], type=pa.int64())})
            pq.write_table(table, data / f"{name}.parquet", row_group_size=2, compression="snappy")
            db.register(name, table)
        pq.write_table(pa.table({"x": pa.array(range(2501), type=pa.int64())}), data / "paging.parquet", row_group_size=500)
        report["fixture_sha256"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in data.glob("*.parquet")}
        connection = trino.dbapi.connect(host="127.0.0.1", port=args.trino_port, user="qualification", request_timeout=30)
        stack.callback(connection.close)
        cursor = connection.cursor()
        report["reference"]["trino"] = cursor.execute("SELECT version()").fetchone()[0]
        coordinator_port = free_port()
        base = f"http://127.0.0.1:{coordinator_port}"
        processes = []
        exchange_token = secrets.token_urlsafe(32)
        analyst_token = secrets.token_urlsafe(32)
        reader_token = secrets.token_urlsafe(32)
        other_token = secrets.token_urlsafe(32)
        headers = {"Authorization": "Bearer " + analyst_token}

        def stop(process):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)

        for index in range(args.workers + 1):
            port = coordinator_port if index == 0 else free_port()
            env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
            env.update({
                "KAVEON_NODE_ID": f"qualification-{index}",
                "KAVEON_ENVIRONMENT": "qualification",
                "KAVEON_COORDINATOR": "true" if index == 0 else "false",
                "KAVEON_HTTP_PORT": str(port),
                "KAVEON_DISCOVERY_URI": base,
                "KAVEON_ADVERTISED_URI": f"http://127.0.0.1:{port}",
                "KAVEON_DATA_DIR": str(data),
                "KAVEON_CATALOG_DIR": str(work / "catalogs"),
                "KAVEON_CATALOG_DATABASE_PATH": str(work / f"catalog-{index}.db"),
                "KAVEON_EXCHANGE_TOKEN": exchange_token,
                "KAVEON_SECURITY_JSON": json.dumps({"principals": [
                    {"token": analyst_token, "principal": "qualification", "role": "analyst"},
                    {"token": reader_token, "principal": "reader", "role": "reader"},
                    {"token": other_token, "principal": "other", "role": "analyst"},
                ]}),
            })
            log = stack.enter_context((args.output / f"node-{index}.log").open("w"))
            process = subprocess.Popen([str(binary), str(work / "absent.toml")], cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            processes.append(process)
            stack.callback(stop, process)
            wait_for(lambda: requests.get(f"http://127.0.0.1:{port}/health", timeout=2).ok, processes)
        wait_for(lambda: requests.get(base + "/v1/cluster", headers=headers, timeout=2).json()["active_workers"] == args.workers, processes)
        security = {
            "anonymous_denied": requests.get(base + "/v1/query", timeout=3).status_code == 401,
            "reader_execution_denied": requests.post(base + "/v1/statement", headers={"Authorization": "Bearer " + reader_token}, json={"query": "SELECT COUNT(*) FROM measurements"}, timeout=3).status_code == 403,
            "public_token_cannot_dispatch_task": requests.post(base + "/v1/task", headers=headers, json={}, timeout=3).status_code == 401,
        }
        report["security"] = security
        paging = {"passed": False}
        try:
            response = requests.post(base + "/v1/statement", headers=headers, json={"query": "SELECT x FROM paging ORDER BY x", "result_delivery": "paged"}, timeout=30)
            response.raise_for_status()
            result = response.json()
            if result.get("state") != "FINISHED" or result.get("error"):
                raise AssertionError(str(result))
            rows = result.get("data") or []
            next_uri = result.get("next_uri")
            page_count = 0
            while next_uri:
                url = base + next_uri
                denied = requests.get(url, headers={"Authorization": "Bearer " + other_token}, timeout=5)
                if denied.status_code not in (403, 404):
                    raise AssertionError("Another principal can read result pages")
                page_response = requests.get(url, headers=headers, timeout=5)
                page_response.raise_for_status()
                page = page_response.json()
                if requests.get(url, headers=headers, timeout=5).json() != page:
                    raise AssertionError("Result page replay changed rows")
                rows.extend(page["data"])
                next_uri = page.get("next_uri")
                page_count += 1
            paging.update(passed=rows == [[i] for i in range(2501)] and page_count >= 3, pages=page_count, rows=len(rows))
        except Exception as error:
            paging["error"] = str(error)
        report["paging"] = paging
        print("PASS paging" if paging["passed"] else "FAIL paging", flush=True)
        cases = BASELINE | (REGRESSIONS if args.regressions else {})
        for name, sql in cases.items():
            case = {"name": name, "sql": sql, "passed": False}
            try:
                expected = [list(row) for row in db.execute(sql).fetchall()]
                trino_rows = [list(row) for row in cursor.execute(reference_sql(sql)).fetchall()]
                case.update(expected=expected, trino=trino_rows)
                if expected != trino_rows:
                    raise AssertionError("Reference engines disagree")
                response = requests.post(base + "/v1/statement", headers=headers, json={"query": sql, "user": "spoofed"}, timeout=30)
                result = response.json()
                case.update(http_status=response.status_code, result=result)
                response.raise_for_status()
                if result.get("error") or result.get("state") != "FINISHED":
                    raise AssertionError("Engine did not finish successfully")
                if result.get("data") != expected:
                    raise AssertionError("Engine rows differ from both reference engines")
                history = requests.get(base + "/v1/query/" + result["id"], headers=headers, timeout=5)
                history.raise_for_status()
                case["identity_preserved"] = history.json()["context"]["principal"] == "qualification"
                if not case["identity_preserved"]:
                    raise AssertionError("Caller-supplied identity displaced authenticated principal")
                other = requests.get(base + "/v1/query/" + result["id"], headers={"Authorization": "Bearer " + other_token}, timeout=5)
                if other.status_code not in (403, 404):
                    raise AssertionError("Another principal can read this query")
                case["stage_count"] = len(history.json().get("stages", []))
                if args.workers and name in BASELINE and not case["stage_count"]:
                    raise AssertionError("Distributed baseline silently executed locally")
                case["passed"] = True
            except Exception as error:
                case["error"] = str(error)
            report["cases"].append(case)
            print(f"{'PASS' if case['passed'] else 'FAIL'} {name}", flush=True)
        if args.concurrency:
            def concurrent_query(_):
                response = requests.post(base + "/v1/statement", headers=headers, json={"query": "SELECT COUNT(*) FROM measurements"}, timeout=30)
                if response.status_code == 429:
                    return {"status": 429, "passed": True, "admitted": False}
                result = response.json()
                return {"status": response.status_code, "passed": response.ok and result.get("data") == [[4]], "admitted": response.ok}
            with ThreadPoolExecutor(max_workers=args.concurrency) as executor:
                report["concurrency"] = list(executor.map(concurrent_query, range(args.concurrency * 3)))
        if args.worker_loss:
            if args.workers < 2:
                raise ValueError("worker-loss requires at least two workers")
            stop(processes[-1])
            response = requests.post(base + "/v1/statement", headers=headers, json={"query": "SELECT COUNT(*) FROM measurements"}, timeout=60)
            result = response.json()
            report["worker_loss"] = {"status": response.status_code, "result": result, "passed": response.ok and result.get("data") == [[4]]}
            print("PASS worker_loss" if report["worker_loss"]["passed"] else "FAIL worker_loss", flush=True)
    report["passed"] = all(case["passed"] for case in report["cases"]) and all(report["security"].values()) and report["paging"]["passed"] and all(case["passed"] for case in report.get("concurrency", [])) and report.get("worker_loss", {}).get("passed", True)
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
