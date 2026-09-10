"""Correctness-gated Kaveon product transaction comparison with PostgreSQL.

The current Kaveon API lacks a bounded product point-read route. Consequently
every generated report sets ``bounded_point_read`` false and cannot pass the
publication gate. Other operations are executable diagnostics once an enabled
ADLS-backed Engine and the disposable PostgreSQL qualification service exist.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import secrets
import statistics
import subprocess
import time

import requests


OPERATIONS = ("point_read", "insert", "update", "delete", "conflicting_update", "multi_record_commit")


def percentile(samples, fraction):
    ordered = sorted(samples)
    return ordered[math.ceil(len(ordered) * fraction) - 1]


def state_hash(rows):
    encoded = json.dumps(sorted(rows, key=lambda row: row[0]), separators=(",", ":"), sort_keys=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def summarize(name, kaveon_ms, postgres_ms, expected_hash, actual_hash):
    passed = bool(kaveon_ms and postgres_ms and expected_hash == actual_hash)
    return {
        "name": name, "samples": min(len(kaveon_ms), len(postgres_ms)), "passed": passed,
        "state_sha256": actual_hash if passed else None,
        "latency_ms": {
            "kaveon": {"median": statistics.median(kaveon_ms), "p95": percentile(kaveon_ms, .95)},
            "postgresql": {"median": statistics.median(postgres_ms), "p95": percentile(postgres_ms, .95)},
        } if kaveon_ms and postgres_ms else None,
    }


def container_details(name):
    value = json.loads(subprocess.check_output(["docker", "inspect", name], text=True))[0]
    host = value["HostConfig"]
    return {"id": value["Id"], "image": value["Image"], "running": value["State"]["Running"],
            "limits": {key: host.get(key) for key in ("NanoCpus", "Memory", "MemorySwap", "CpusetCpus", "CpuQuota", "CpuPeriod")}}


class Kaveon:
    def __init__(self, endpoint, token, verify=True):
        self.endpoint = endpoint.rstrip("/")
        self.session = requests.Session()
        self.session.headers["Authorization"] = "Bearer " + token
        self.session.verify = verify

    def sql(self, statement, transaction_id=None):
        response = self.session.post(self.endpoint + "/v1/transaction/sql",
                                     json={"sql": statement, "transaction_id": transaction_id}, timeout=120)
        response.raise_for_status()
        return response.json() if response.content else None

    def begin(self):
        result = self.sql("BEGIN")
        return result["transaction_id"], result["base"]

    def commit(self, transaction_id):
        return self.sql("COMMIT", transaction_id)

    def rollback(self, transaction_id):
        response = self.session.post(self.endpoint + "/v1/transaction/sql",
                                     json={"sql": "ROLLBACK", "transaction_id": transaction_id}, timeout=120)
        response.raise_for_status()

    def create(self, transaction_id, record_id, document):
        value = json.dumps(document, separators=(",", ":")).replace("'", "''")
        self.sql(f"INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('{record_id}', '{value}')", transaction_id)

    def update(self, transaction_id, record_id, revision, document):
        value = json.dumps(document, separators=(",", ":")).replace("'", "''")
        self.sql(f"UPDATE kaveon.product.datasets SET document_json = '{value}' WHERE id = '{record_id}' AND revision = {revision}", transaction_id)

    def delete(self, transaction_id, record_id, revision):
        self.sql(f"DELETE FROM kaveon.product.datasets WHERE id = '{record_id}' AND revision = {revision}", transaction_id)

    def state(self, prefix):
        transaction_id, base = self.begin()
        try:
            rows = []
            for key, record in base.get("product_records", {}).items():
                if key.startswith("dataset/" + prefix):
                    rows.append([record["id"], record["revision"], record["document"]["sha256"]])
            return rows
        finally:
            self.rollback(transaction_id)


class Postgres:
    def __init__(self, dsn, table):
        try:
            import psycopg2
        except ImportError as error:
            raise RuntimeError(
                "psycopg2 is required only for a live PostgreSQL comparison run"
            ) from error
        self._driver = psycopg2
        self.dsn, self.table = dsn, table
        self.connection = self.connect()
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'CREATE TABLE "{table}" (id TEXT PRIMARY KEY, revision BIGINT NOT NULL, document JSONB NOT NULL, document_sha256 TEXT NOT NULL)')

    def connect(self):
        return self._driver.connect(self.dsn)

    def create(self, record_id, document):
        digest = document_hash(document)
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'INSERT INTO "{self.table}" VALUES (%s,1,%s,%s)', (record_id, json.dumps(document), digest))

    def update(self, record_id, revision, document):
        digest = document_hash(document)
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'UPDATE "{self.table}" SET revision=revision+1,document=%s,document_sha256=%s WHERE id=%s AND revision=%s',
                           (json.dumps(document), digest, record_id, revision))
            if cursor.rowcount != 1: raise RuntimeError("PostgreSQL revision conflict")

    def delete(self, record_id, revision):
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'DELETE FROM "{self.table}" WHERE id=%s AND revision=%s', (record_id, revision))
            if cursor.rowcount != 1: raise RuntimeError("PostgreSQL revision conflict")

    def multi_create(self, items):
        with self.connection as connection, connection.cursor() as cursor:
            for record_id, document in items:
                cursor.execute(f'INSERT INTO "{self.table}" VALUES (%s,1,%s,%s)',
                               (record_id, json.dumps(document), document_hash(document)))

    def state(self, prefix):
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'SELECT id,revision,document_sha256 FROM "{self.table}" WHERE id LIKE %s ORDER BY id', (prefix + "%",))
            return [list(row) for row in cursor.fetchall()]

    def close(self):
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute(f'DROP TABLE "{self.table}"')
        self.connection.close()

    def version(self):
        with self.connection as connection, connection.cursor() as cursor:
            cursor.execute("SELECT version()")
            return cursor.fetchone()[0]


def document_hash(document):
    return hashlib.sha256(json.dumps(document, separators=(",", ":"), sort_keys=True).encode()).hexdigest()


def timed(call):
    start = time.perf_counter(); call(); return (time.perf_counter() - start) * 1000


def run(args):
    containers = {"kaveon": container_details(args.kaveon_container),
                  "postgresql": container_details(args.postgres_container)}
    limits = {name: details["limits"] for name, details in containers.items()}
    resources_matched = (limits["kaveon"] == limits["postgresql"]
                         and all(details["running"] for details in containers.values()))
    token = os.getenv(args.token_env)
    dsn = os.getenv(args.postgres_dsn_env)
    if not token or not dsn:
        raise RuntimeError(f"set {args.token_env} and {args.postgres_dsn_env}")
    prefix = "bench_" + secrets.token_hex(6) + "_"
    kaveon = Kaveon(args.kaveon_url, token, args.ca_cert or True)
    postgres = Postgres(dsn, prefix + "records")
    samples = {name: {"kaveon": [], "postgresql": []} for name in OPERATIONS}
    try:
        for index in range(args.warmups + args.repetitions):
            measured = index >= args.warmups
            # Insert, update, delete and a three-record atomic commit use the same IDs/documents.
            insert_id, update_id, delete_id = (f"{prefix}{label}_{index}" for label in ("insert", "update", "delete"))
            for engine in (("kaveon", "postgresql") if index % 2 == 0 else ("postgresql", "kaveon")):
                if engine == "kaveon":
                    elapsed = timed(lambda: kaveon_insert(kaveon, insert_id, {"value": index}))
                else: elapsed = timed(lambda: postgres.create(insert_id, {"value": index}))
                if measured: samples["insert"][engine].append(elapsed)
            kaveon_insert(kaveon, update_id, {"value": 0}); postgres.create(update_id, {"value": 0})
            kaveon_insert(kaveon, delete_id, {"value": 0}); postgres.create(delete_id, {"value": 0})
            for engine in (("postgresql", "kaveon") if index % 2 == 0 else ("kaveon", "postgresql")):
                elapsed = timed(lambda: kaveon_update(kaveon, update_id, {"value": index})) if engine == "kaveon" else timed(lambda: postgres.update(update_id, 1, {"value": index}))
                if measured: samples["update"][engine].append(elapsed)
                elapsed = timed(lambda: kaveon_delete(kaveon, delete_id)) if engine == "kaveon" else timed(lambda: postgres.delete(delete_id, 1))
                if measured: samples["delete"][engine].append(elapsed)
            items = [(f"{prefix}multi_{index}_{n}", {"value": n}) for n in range(3)]
            elapsed = timed(lambda: kaveon_multi(kaveon, items));
            if measured: samples["multi_record_commit"]["kaveon"].append(elapsed)
            elapsed = timed(lambda: postgres.multi_create(items));
            if measured: samples["multi_record_commit"]["postgresql"].append(elapsed)
        # Point-read is a diagnostic full-snapshot transfer in Kaveon today.
        for _ in range(args.repetitions):
            samples["point_read"]["kaveon"].append(timed(lambda: kaveon.state(prefix)))
            samples["point_read"]["postgresql"].append(timed(lambda: postgres.state(prefix)))
        # Conflict correctness is required but timed as one paired optimistic race per sample.
        conflict_passed = True
        for index in range(args.repetitions):
            record_id = f"{prefix}conflict_{index}"; kaveon_insert(kaveon, record_id, {"value": 0}); postgres.create(record_id, {"value": 0})
            start = time.perf_counter(); conflict_passed &= kaveon_conflict(kaveon, record_id)
            samples["conflicting_update"]["kaveon"].append((time.perf_counter()-start)*1000)
            start = time.perf_counter(); conflict_passed &= postgres_conflict(postgres, record_id)
            samples["conflicting_update"]["postgresql"].append((time.perf_counter()-start)*1000)
        expected, actual = postgres.state(prefix), kaveon.state(prefix)
        expected_hash, actual_hash = state_hash(expected), state_hash(actual)
        operations = [summarize(name, samples[name]["kaveon"], samples[name]["postgresql"], expected_hash, actual_hash) for name in OPERATIONS]
        next(item for item in operations if item["name"] == "conflicting_update")["passed"] &= conflict_passed
        total_k = sum(sum(value["kaveon"]) for value in samples.values()) / 1000
        total_p = sum(sum(value["postgresql"]) for value in samples.values()) / 1000
        count = sum(len(value["kaveon"]) for value in samples.values())
        p95_k = statistics.median([item["latency_ms"]["kaveon"]["p95"] for item in operations])
        p95_p = statistics.median([item["latency_ms"]["postgresql"]["p95"] for item in operations])
        return {"schema_version": 1, "started_at": datetime.now(timezone.utc).isoformat(),
                "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "run_prefix": prefix,
                "containers": containers, "resources_matched": resources_matched,
                "versions": {"postgresql": postgres.version()},
                "bounded_point_read": False,
                "point_read_limitation": "Kaveon BEGIN returns the complete base snapshot; no bounded product point-read HTTP route exists.",
                "publication_workload_gate": args.warmups >= 5 and args.repetitions >= 30 and resources_matched,
                "correctness_passed": all(item["passed"] for item in operations), "operations": operations,
                "concurrency": {"kaveon": 1, "postgresql": 1}, "concurrency_matched": True,
                "warmups": args.warmups, "repetitions": args.repetitions,
                "kaveon_over_postgresql_qps": (count/total_k)/(count/total_p),
                "kaveon_over_postgresql_p95_ratio": p95_k/p95_p}
    finally:
        try:
            for record_id, revision, _ in kaveon.state(prefix):
                kaveon_delete(kaveon, record_id, revision)
        except Exception:
            # Preserve the primary result/error; the random prefix is recorded
            # implicitly in retained IDs for explicit cleanup investigation.
            pass
        postgres.close()


def kaveon_insert(client, record_id, document):
    tx, _ = client.begin(); client.create(tx, record_id, document); client.commit(tx)
def kaveon_update(client, record_id, document):
    tx, _ = client.begin(); client.update(tx, record_id, 1, document); client.commit(tx)
def kaveon_delete(client, record_id, revision=1):
    tx, _ = client.begin(); client.delete(tx, record_id, revision); client.commit(tx)
def kaveon_multi(client, items):
    tx, _ = client.begin()
    for record_id, document in items: client.create(tx, record_id, document)
    client.commit(tx)
def kaveon_conflict(client, record_id):
    left, _ = client.begin(); right, _ = client.begin()
    client.update(left, record_id, 1, {"value": 1}); client.update(right, record_id, 1, {"value": 1})
    outcomes = []
    for tx in (left, right):
        try: client.commit(tx); outcomes.append("success")
        except requests.HTTPError as error: outcomes.append("conflict" if error.response.status_code == 409 else "error")
    return sorted(outcomes) == ["conflict", "success"]
def postgres_conflict(client, record_id):
    def attempt(_):
        connection = client.connect()
        try:
            digest = document_hash({"value": 1})
            with connection, connection.cursor() as cursor:
                cursor.execute(f'UPDATE "{client.table}" SET revision=revision+1,document=%s,document_sha256=%s WHERE id=%s AND revision=1',
                               (json.dumps({"value": 1}), digest, record_id))
                return cursor.rowcount == 1
        finally:
            connection.close()
    with ThreadPoolExecutor(max_workers=2) as executor:
        outcomes = list(executor.map(attempt, (1, 2)))
    return sorted(outcomes) == [False, True]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kaveon-url", default="https://127.0.0.1:18443")
    parser.add_argument("--ca-cert", help="private CA PEM for the Kaveon HTTPS endpoint")
    parser.add_argument("--token-env", default="KAVEON_QUALIFICATION_TOKEN")
    parser.add_argument("--postgres-dsn-env", default="KAVEON_QUALIFICATION_POSTGRES_DSN")
    parser.add_argument("--kaveon-container", required=True); parser.add_argument("--postgres-container", required=True)
    parser.add_argument("--warmups", type=int, default=5); parser.add_argument("--repetitions", type=int, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.warmups < 1 or args.repetitions < 1:
        parser.error("warmups and repetitions must be positive")
    report = run(args); args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{'PASS' if report['correctness_passed'] else 'FAIL'} diagnostic; publication blocked by unbounded Kaveon point read")
    return 2

if __name__ == "__main__": raise SystemExit(main())
