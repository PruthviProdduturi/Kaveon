"""Same-file exact-result checks and fixed-workload timings; Docker mode matches resource caps."""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
import math
import platform
import re
from pathlib import Path
import secrets
import shutil
import statistics
import subprocess
import tempfile
import time

QUERIES = {
    "filtered_sum": "SELECT COUNT(*), SUM(amount) FROM events WHERE amount > 500",
    "grouped_sum": "SELECT category, COUNT(*), SUM(amount) FROM events GROUP BY category ORDER BY category",
    "join": "SELECT COUNT(*), SUM(e.amount) FROM events e JOIN customers c ON e.customer_id=c.customer_id",
    "small_left_join": "SELECT COUNT(*), SUM(e.amount) FROM customers c JOIN events e ON c.customer_id=e.customer_id",
    "topn": "SELECT event_id, amount FROM events ORDER BY amount DESC, event_id LIMIT 20",
    "distinct": "SELECT COUNT(DISTINCT customer_id) FROM events",
}

EXTENDED_QUERIES = {
    **QUERIES,
    "unfiltered_count": "SELECT COUNT(*) FROM events",
    "arithmetic_projection": "SELECT event_id, amount * 2 + 1 AS adjusted FROM events WHERE event_id < 10000 ORDER BY event_id",
    "medium_groups": "SELECT customer_id % 1024 AS bucket, COUNT(*), SUM(amount) FROM events GROUP BY customer_id % 1024 ORDER BY bucket",
    "high_groups": "SELECT customer_id, COUNT(*), SUM(amount) FROM events GROUP BY customer_id ORDER BY customer_id",
    "multi_aggregate": "SELECT category, COUNT(*), SUM(amount), MIN(amount), MAX(amount), AVG(amount) FROM events GROUP BY category ORDER BY category",
    "grouped_join": "SELECT e.category, COUNT(*), SUM(e.amount) FROM events e JOIN customers c ON e.customer_id=c.customer_id GROUP BY e.category ORDER BY e.category",
}


def workspace_provenance():
    """Record source state at invocation, without pretending it proves image provenance."""
    root = Path(__file__).resolve().parents[2]
    names = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "engine"],
        cwd=root,
    ).decode().split("\0")
    hashes = {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
              for name in sorted(set(names)) if name and (root / name).is_file()}
    return {"commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
            "engine_source_files": hashes,
            "engine_source_manifest_sha256": hashlib.sha256(json.dumps(hashes, sort_keys=True).encode()).hexdigest(),
            "limitation": "Invocation workspace snapshot only; image ID identifies the executed build, but its source correspondence is not independently established."}


def resource_limits(container):
    config = container["HostConfig"]
    return {key: config.get(key) for key in ["NanoCpus", "Memory", "MemorySwap", "CpusetCpus", "CpuQuota", "CpuPeriod"]}


def hardware_details():
    if os.name != "nt":
        return {"limitation": "Supply storage-device and exact CPU model details when publishing from this platform."}
    drive = Path(__file__).resolve().drive[:1]
    if not drive.isalpha():
        return {"limitation": "Workspace is not on a local Windows drive."}
    script = "@{processors=@(Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors); storage=@(Get-Partition -DriveLetter " + drive + " | Get-Disk | Select-Object FriendlyName,BusType,Size)} | ConvertTo-Json -Depth 4"
    try:
        return json.loads(subprocess.check_output(["powershell", "-NoProfile", "-Command", script], text=True, timeout=30))
    except (subprocess.SubprocessError, json.JSONDecodeError) as error:
        return {"limitation": f"Hardware inspection failed: {type(error).__name__}"}


def redact_config(text):
    return "\n".join(line.split("=", 1)[0] + "=<redacted>" if "=" in line and re.search(r"password|secret|token|credential|access.?key", line.split("=", 1)[0], re.I) else line for line in text.splitlines())


def main():
    # Keep the workload contract importable by the fail-closed claim evaluator
    # without requiring benchmark-only native dependencies during collection.
    global duckdb, pa, pq, requests, psutil, trino, free_port, wait_for
    import duckdb
    import pyarrow as pa
    import pyarrow.parquet as pq
    import requests
    import psutil
    import trino
    from smoke import free_port, wait_for

    parser = argparse.ArgumentParser(description=__doc__)
    runner = parser.add_mutually_exclusive_group(required=True)
    runner.add_argument("--server-bin", type=Path)
    runner.add_argument("--docker-image", help="Release image to run with the same 4 CPU/8 GiB limits as Trino")
    parser.add_argument("--rows", type=int, default=100_000)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--customers", type=int, default=10_000)
    parser.add_argument("--suite", choices=["diagnostic", "extended"], default="diagnostic")
    parser.add_argument("--local-parallelism", type=int, choices=[1, 2, 4], default=1)
    parser.add_argument("--throughput-rounds", type=int, default=0, help="Mixed-workload rounds per engine; zero skips throughput")
    parser.add_argument("--throughput-repeats", type=int, default=10, help="Executions of every query per finite-batch throughput round")
    parser.add_argument("--concurrency", type=int, choices=[1, 2, 4], default=4)
    parser.add_argument("--workers", type=int, choices=[0, 2, 5], default=0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    queries = EXTENDED_QUERIES if args.suite == "extended" else QUERIES
    corpus_sha256 = hashlib.sha256(json.dumps(queries, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    if args.warmups < 1 or args.customers < 1:
        parser.error("warmups and customers must be positive")
    if args.rows < 1 or args.repetitions < 1 or args.throughput_rounds < 0 or args.throughput_repeats < 1:
        parser.error("rows, repetitions and throughput repeats must be positive; throughput rounds must be nonnegative")
    if args.docker_image and args.workers:
        parser.error("matched Docker comparison currently supports single-node topology only")
    binary = args.server_bin.resolve(strict=True) if args.server_bin else None
    args.output.mkdir(parents=True, exist_ok=True)
    query_memory_bytes = 1024**3 if args.throughput_rounds else 4 * 1024**3
    if binary:
        frozen_binary = args.output.resolve() / ("qualified-" + binary.name)
        shutil.copy2(binary, frozen_binary)
        binary = frozen_binary
    # This location is mounted read-only into the qualification Trino service.
    lake = Path(__file__).resolve().parents[2] / "tmp" / "qualification-lake"
    run = "run_" + secrets.token_hex(6)
    data = lake / run
    data.mkdir(parents=True)
    report = {"same_file_correctness": True, "fair_performance_comparison": False,
              "limitation": "Kaveon runs natively; Trino runs in Docker with 4 CPUs/8 GiB. Timings cannot establish a relative performance score.",
              "workers": args.workers, "local_parallelism": args.local_parallelism, "rows": args.rows, "run": run, "cases": [],
              "suite": args.suite, "warmups_per_query": args.warmups,
              "query_corpus": {"names": list(queries), "sha256": corpus_sha256},
              "cache_policy": {"primary": "warm", "warmup_executions_per_query": args.warmups,
                               "cold_cache": "excluded_from_primary_and_must_be_run_as_a_separate_matched experiment",
                               "reason": "portable user-space cache eviction cannot prove equivalent OS, filesystem, JVM and Engine cache state"},
              "customers": min(args.customers, args.rows),
              "publication_workload_gate": args.suite == "extended" and args.rows >= 5_000_000 and min(args.customers, args.rows) >= 100_000 and args.warmups >= 5 and args.repetitions >= 30,
              "kaveon_query_memory_bytes": query_memory_bytes if args.docker_image else None,
              "kaveon_admission_memory_bytes": 6 * 1024**3 if args.docker_image else None,
              "started_at": datetime.now(timezone.utc).isoformat(),
              "host": {"os": platform.platform(), "processor": platform.processor(), "logical_cpus": psutil.cpu_count(), "physical_cores": psutil.cpu_count(logical=False), "ram_bytes": psutil.virtual_memory().total},
              "hardware_details": hardware_details(),
              "dataset": {"generator_version": 1, "compression": "snappy", "row_group_rows": 16384, "file_count": 2},
              "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "workspace_at_invocation": workspace_provenance(),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest() if binary else None,
              "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()}
    if args.docker_image:
        report["docker_image"] = json.loads(subprocess.check_output(["docker", "image", "inspect", args.docker_image], text=True))[0]["Id"]
        reference = json.loads(subprocess.check_output(["docker", "inspect", "kaveon-qualification-trino-1"], text=True))[0]
        if reference["HostConfig"]["NanoCpus"] != 4_000_000_000 or reference["HostConfig"]["Memory"] != 8 * 1024**3:
            raise RuntimeError("Trino resource limits differ from the required 4 CPUs/8 GiB")
        bindings = reference["NetworkSettings"]["Ports"].get("8080/tcp") or []
        if not reference["State"]["Running"] or not any(binding["HostPort"] == "18080" for binding in bindings):
            raise RuntimeError("The inspected Trino container is not the running reference on port 18080")
        report["trino_runtime_limits"] = resource_limits(reference)
        report["trino_configuration"] = {name: redact_config(subprocess.check_output(["docker", "exec", reference["Id"], "cat", "/etc/trino/" + name], text=True)) for name in ["config.properties", "node.properties", "jvm.config", "catalog/lake.properties"]}
        report.update(fair_performance_comparison=True, limitation="Single-node warm-cache workload; results apply only to these queries/data. Both services have 4 CPUs/8 GiB, but their internal memory policies differ.", resources={"cpus":4,"memory_bytes":8*1024**3}, trino_image=reference["Image"])
    with contextlib.ExitStack() as stack:
        db = stack.enter_context(duckdb.connect())
        customers = min(args.customers, args.rows)
        tables = {
            "events": db.execute(f"SELECT i::BIGINT AS event_id, (i % {customers})::BIGINT AS customer_id, (i % 17)::BIGINT AS category, (i % 1000)::BIGINT AS amount FROM range({args.rows}) t(i)").fetch_arrow_table(),
            "customers": db.execute(f"SELECT i::BIGINT AS customer_id FROM range({customers}) t(i)").fetch_arrow_table(),
        }
        connection = trino.dbapi.connect(host="127.0.0.1", port=18080, user="qualification", catalog="lake", schema=run, request_timeout=120)
        stack.callback(connection.close)
        cursor = connection.cursor()
        cursor.execute(f"CREATE SCHEMA lake.{run}").fetchall()
        report["versions"] = {"trino": cursor.execute("SELECT version()").fetchone()[0], "duckdb": duckdb.__version__, "pyarrow": pa.__version__}
        report["trino_active_nodes"] = cursor.execute("SELECT COUNT(*) FROM system.runtime.nodes WHERE state = 'active'").fetchone()[0]
        if args.docker_image and report["trino_active_nodes"] != 1:
            raise RuntimeError("Matched single-node comparison requires exactly one active Trino node")
        report["files"] = {}
        for name, table in tables.items():
            folder = data / name
            (folder / "_delta_log").mkdir(parents=True)
            file = folder / "data.parquet"
            pq.write_table(table, file, compression="snappy", row_group_size=16_384)
            schema = {"type": "struct", "fields": [{"name": field.name, "type": "long", "nullable": True, "metadata": {}} for field in table.schema]}
            actions = [{"protocol": {"minReaderVersion": 1, "minWriterVersion": 2}},
                       {"metaData": {"id": secrets.token_hex(16), "format": {"provider": "parquet", "options": {}}, "schemaString": json.dumps(schema), "partitionColumns": [], "configuration": {}}},
                       {"add": {"path": "data.parquet", "partitionValues": {}, "size": file.stat().st_size, "modificationTime": 0, "dataChange": True}}]
            (folder / "_delta_log" / "00000000000000000000.json").write_text("\n".join(json.dumps(action) for action in actions), encoding="utf-8")
            report["files"][name] = {"path": str(file), "sha256": hashlib.sha256(file.read_bytes()).hexdigest(), "bytes": file.stat().st_size}
            db.execute(f"CREATE VIEW {name} AS SELECT * FROM read_parquet('{file.as_posix()}')")
            columns = ", ".join(f"{field.name} BIGINT" for field in table.schema)
            cursor.execute(f"CREATE TABLE lake.{run}.{name} ({columns}) WITH (format='PARQUET', external_location='local:///qualification-lake/{run}/{name}')").fetchall()
        work = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="kaveon-same-files-")))
        (work / "catalogs").mkdir()
        base_port = free_port()
        base = f"http://127.0.0.1:{base_port}"
        token, exchange_token = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        headers = {"Authorization": "Bearer " + token}
        processes = []

        def stop(process):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)

        for index in range(args.workers + 1):
            port = base_port if index == 0 else free_port()
            env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
            env.update({"KAVEON_NODE_ID": f"same-files-{index}", "KAVEON_ENVIRONMENT": "qualification",
                        "KAVEON_COORDINATOR": str(index == 0).lower(), "KAVEON_HTTP_PORT": str(port),
                        "KAVEON_DISCOVERY_URI": base, "KAVEON_ADVERTISED_URI": f"http://127.0.0.1:{port}",
                        "KAVEON_DATA_DIR": str(data), "KAVEON_CATALOG_DIR": str(work / "catalogs"),
                        "KAVEON_CATALOG_DATABASE_PATH": str(work / f"catalog-{index}.db"),
                        "KAVEON_EXCHANGE_TOKEN": exchange_token,
                        "KAVEON_LOCAL_PARALLELISM": str(args.local_parallelism),
                        "KAVEON_SECURITY_JSON": json.dumps({"principals": [{"token": token, "principal": "qualification", "role": "analyst"}]})})
            log = stack.enter_context((args.output / f"node-{index}.log").open("w"))
            if args.docker_image:
                container_name = "kaveon-benchmark-" + run
                container_env = {key: value for key, value in env.items() if key.startswith("KAVEON_")}
                container_env.update(KAVEON_HTTP_PORT="8080", KAVEON_BIND_HOST="0.0.0.0", KAVEON_INSECURE_DEVELOPMENT="true", KAVEON_DATA_DIR="/data", KAVEON_CATALOG_DIR="/tmp/catalogs", KAVEON_CATALOG_DATABASE_PATH="/tmp/catalog.db", KAVEON_QUERY_MEMORY_LIMIT_BYTES=str(query_memory_bytes), KAVEON_MEMORY_ADMISSION_LIMIT_BYTES=str(6*1024**3))
                report["kaveon_configuration"] = {key: "<redacted>" if key in {"KAVEON_SECURITY_JSON", "KAVEON_EXCHANGE_TOKEN"} else value for key, value in container_env.items()}
                report["kaveon_configuration"]["security_policy"] = {"principal": "qualification", "role": "analyst", "random_per_run_tokens": True}
                command = ["docker", "run", "--rm", "--name", container_name, "--cpus", "4", "--memory", "8g", "-p", f"127.0.0.1:{port}:8080", "--mount", f"type=bind,source={data},target=/data,readonly"]
                for key, value in container_env.items():
                    command.extend(["-e", f"{key}={value}"])
                command.append(report["docker_image"])
            else:
                command = [str(binary), str(work / "absent.toml")]
            process = subprocess.Popen(command, cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            processes.append(process)
            stack.callback(stop, process)
            if args.docker_image:
                stack.callback(lambda name=container_name: subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False))
            wait_for(lambda: requests.get(f"http://127.0.0.1:{port}/health", timeout=2).ok, processes, timeout=90)
            if args.docker_image:
                running = json.loads(subprocess.check_output(["docker", "inspect", container_name], text=True))[0]
                report["kaveon_runtime_limits"] = resource_limits(running)
                if running["Image"] != report["docker_image"] or running["HostConfig"]["NanoCpus"] != 4_000_000_000 or running["HostConfig"]["Memory"] != 8 * 1024**3:
                    raise RuntimeError("Kaveon runtime image or resource caps differ from the recorded benchmark configuration")
                if report["kaveon_runtime_limits"] != report["trino_runtime_limits"]:
                    raise RuntimeError("Runtime CPU, memory, affinity or swap limits differ between engines")
        wait_for(lambda: requests.get(base + "/v1/cluster", headers=headers, timeout=2).json()["active_workers"] == args.workers, processes)
        expected_rows = {name: [list(row) for row in db.execute(sql).fetchall()] for name, sql in queries.items()}
        for name, sql in queries.items():
            case = {"name": name, "sql": sql, "passed": False, "trino_ms": [], "kaveon_ms": []}
            try:
                expected = [list(row) for row in db.execute(sql).fetchall()]
                # Alternate execution order through warmups and measured executions.
                for repetition in range(args.repetitions + args.warmups):
                    for engine in (["trino", "kaveon"] if repetition % 2 == 0 else ["kaveon", "trino"]):
                        started = time.perf_counter()
                        if engine == "trino":
                            actual = [list(row) for row in cursor.execute(sql).fetchall()]
                        else:
                            response = requests.post(base + "/v1/statement", headers=headers, json={"query": sql, "result_delivery": "paged"}, timeout=120)
                            result = response.json()
                            if not response.ok:
                                raise AssertionError(f"HTTP {response.status_code}: {result}")
                            if result.get("error") or result.get("state") != "FINISHED":
                                raise AssertionError(str(result))
                            actual = result.get("data") or []
                            next_uri = result.get("next_uri")
                            while next_uri:
                                page_response = requests.get(base + next_uri, headers=headers, timeout=30)
                                page_response.raise_for_status()
                                page = page_response.json()
                                actual.extend(page["data"])
                                next_uri = page.get("next_uri")
                            # Paging supports replay until explicit release; consume and release
                            # every result so a long benchmark does not exhaust retained-result quota.
                            requests.delete(base + "/v1/query/" + result["id"], headers=headers, timeout=30).raise_for_status()
                        elapsed = (time.perf_counter() - started) * 1000
                        if engine == "kaveon":
                            history = requests.get(base + "/v1/query/" + result["id"], headers=headers, timeout=5).json()
                            case["stages"] = len(history.get("stages", []))
                            if args.workers and not case["stages"]:
                                raise AssertionError("Distributed query silently fell back to local")
                        if actual != expected:
                            raise AssertionError(f"{engine} rows disagree with DuckDB reading the same Parquet files")
                        if repetition >= args.warmups:
                            case[engine + "_ms"].append(elapsed)
                case["passed"] = True
                case["median_ms"] = {engine: statistics.median(case[engine + "_ms"]) for engine in ["trino", "kaveon"]}
                encoded_result = json.dumps(expected, separators=(",", ":")).encode()
                case["result_rows"] = len(expected)
                case["result_sha256"] = hashlib.sha256(encoded_result).hexdigest()
                case["statistics"] = {}
                for engine in ["trino", "kaveon"]:
                    samples = sorted(case[engine + "_ms"])
                    median_seconds = statistics.median(samples) / 1000
                    case["statistics"][engine] = {
                        "min_ms": samples[0], "max_ms": samples[-1],
                        "p95_ms": samples[math.ceil(len(samples) * 0.95) - 1],
                        "p95_method": "nearest rank",
                        "result_rows_per_second_at_median": len(expected) / median_seconds,
                        "result_json_bytes_per_second_at_median": len(encoded_result) / median_seconds,
                    }
            except Exception as error:
                case["error"] = str(error)
            report["cases"].append(case)
            print(f"{'PASS' if case['passed'] else 'FAIL'} {name}", flush=True)
        if args.throughput_rounds:
            throughput = {"concurrency": args.concurrency, "rounds": args.throughput_rounds,
                          "queries_per_round": len(queries) * args.throughput_repeats,
                          "repetitions_per_query_per_round": args.throughput_repeats, "target_ratio": 1.9,
                          "metric": "successful exact-result queries per second on finite batches of the fixed equal-weight six-query workload",
                          "scope": "Observed aggregate throughput ratio for this fixture, including client submission, complete result retrieval and correctness checks; not engine-wide superiority or a statistical confidence bound.",
                          "connection_policy": "Fresh HTTP session per query for both engines; no cross-thread connection sharing.",
                          "result_lifecycle": "Consume all pages and explicitly release Kaveon replay state inside query timing; Trino releases consumed results through its protocol.",
                          "warmup": {},
                          "trino": [], "kaveon": [], "passed": True}

            def execute_checked(engine, item):
                name, sql = item
                started = time.perf_counter()
                try:
                    if engine == "trino":
                        conn = trino.dbapi.connect(host="127.0.0.1", port=18080, user="qualification", catalog="lake", schema=run, request_timeout=120)
                        try:
                            actual = [list(row) for row in conn.cursor().execute(sql).fetchall()]
                        finally:
                            conn.close()
                    else:
                        with requests.Session() as session:
                            response = session.post(base + "/v1/statement", headers=headers, json={"query": sql, "result_delivery": "paged"}, timeout=120)
                            if not response.ok:
                                raise AssertionError(f"HTTP {response.status_code}: {response.text[:2000]}")
                            result = response.json()
                            if result.get("error") or result.get("state") != "FINISHED":
                                raise AssertionError(str(result))
                            actual = result.get("data") or []
                            next_uri = result.get("next_uri")
                            while next_uri:
                                response = session.get(base + next_uri, headers=headers, timeout=30)
                                response.raise_for_status()
                                page = response.json()
                                actual.extend(page["data"])
                                next_uri = page.get("next_uri")
                            session.delete(base + "/v1/query/" + result["id"], headers=headers, timeout=30).raise_for_status()
                    if actual != expected_rows[name]:
                        raise AssertionError("Rows disagree with DuckDB")
                    return {"name": name, "passed": True, "ms": (time.perf_counter() - started) * 1000}
                except Exception as error:
                    return {"name": name, "passed": False, "error": str(error), "ms": (time.perf_counter() - started) * 1000}

            with ThreadPoolExecutor(max_workers=args.concurrency) as executor:
                # Warm the concurrent path and all executor threads before starting measured batches.
                for engine in ["trino", "kaveon"]:
                    futures = [executor.submit(execute_checked, engine, item) for item in list(queries.items()) * args.warmups]
                    results = [future.result() for future in futures]
                    throughput["warmup"][engine] = results
                    throughput["passed"] &= all(item["passed"] for item in results)
                for round_index in range(args.throughput_rounds):
                    for engine in (["trino", "kaveon"] if round_index % 2 == 0 else ["kaveon", "trino"]):
                        started = time.perf_counter()
                        # Identical deterministic rotation distributes long queries across each round.
                        items = list(queries.items())
                        offset = round_index % len(items)
                        items = (items[offset:] + items[:offset]) * args.throughput_repeats
                        futures = [executor.submit(execute_checked, engine, item) for item in items]
                        results = [future.result() for future in futures]
                        seconds = time.perf_counter() - started
                        passed = all(item["passed"] for item in results)
                        throughput["passed"] &= passed
                        throughput[engine].append({"round": round_index + 1, "seconds": seconds, "results": results,
                                                   "successful_qps": sum(item["passed"] for item in results) / seconds})
                        print(f"{'PASS' if passed else 'FAIL'} throughput {engine} round {round_index + 1}", flush=True)
            throughput["aggregate_qps"] = {engine: sum(sum(item["passed"] for item in sample["results"]) for sample in throughput[engine]) / sum(sample["seconds"] for sample in throughput[engine]) for engine in ["trino", "kaveon"]}
            reference_qps = throughput["aggregate_qps"]["trino"]
            throughput["kaveon_over_trino"] = throughput["aggregate_qps"]["kaveon"] / reference_qps if reference_qps else None
            throughput["paired_round_ratios"] = [
                candidate["successful_qps"] / reference["successful_qps"] if reference["successful_qps"] else None
                for candidate, reference in zip(throughput["kaveon"], throughput["trino"], strict=True)
            ]
            throughput["target_met"] = bool(report["publication_workload_gate"] and report["fair_performance_comparison"] and all(case["passed"] for case in report["cases"]) and throughput["passed"] and throughput["kaveon_over_trino"] is not None and throughput["kaveon_over_trino"] >= 1.9)
            report["throughput"] = throughput
    report["passed"] = all(case["passed"] for case in report["cases"]) and report.get("throughput", {}).get("passed", True)
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    (args.output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
