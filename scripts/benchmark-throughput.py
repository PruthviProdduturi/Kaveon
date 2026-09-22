"""The throughput tier: N concurrent clients run a suite's statements on one
engine, alone on the worker nodes, and the figure is successful exact
executions per second. The same script serves both engines so the two sides
are measured by identical code: ENGINE=kaveon executes through the API's
Engine bridge (as scripts/scale-suite.py does, inside a Job built from the
API pod contract by scripts/aks-scale-suite-job.py --script); ENGINE=trino
executes through Trino's HTTP statement API (the client duplicated from
scripts/benchmark-trino-suite.py so the Job stays a single mounted file).

Each client is a thread that runs the suite's statements in a fixed
permutation seeded by its client index, looping until the duration ends or
the round count is reached. Every execution is timed and its result digest
(scripts/scale-suite.py's rendering, floating-point values to nine
significant digits, rows sorted) is checked against the first digest seen
for that statement in this run; a different digest is a failed execution,
except that an `ORDER BY … LIMIT` statement returning a different set of
rows of the same size broke a tie at the cut differently and is counted as
a tie beside the execution. Admission refusals (Kaveon answers 429
MEMORY_ADMISSION_REJECTED, which the bridge surfaces as HTTPException 429;
Trino QUERY_QUEUE_FULL) are not wrong results: they are counted apart as
rejections, retried after a short backoff, and cost the engine the time they
took. A warm-up phase of the same shape precedes the measured window and is
reported separately.

Output: a progress line every 30 s, one JSON summary per statement (count,
failures, rejections, p50/p95/max seconds), and a final THROUGHPUT= line
with the clients, the window, successful exact executions, failures,
rejections, executions per second, per-client counts and the statements
that failed in every attempt. OUTPUT names a file for the same JSON.

Environment: ENGINE (kaveon | trino); SUITE (suite JSON); CLIENTS;
DURATION_SECONDS and/or ROUNDS (at least one; the window ends at whichever
comes first); WARMUP_SECONDS (default 30); STATEMENT_TIMEOUT_SECONDS
(default 900); ENGINE_DIGEST (recorded verbatim); OUTPUT (optional). For
Trino also TRINO_URL, /trino-tls/ca.crt, /trino-auth/client-password,
TRINO_TABLES and TRINO_ACCOUNT as in scripts/benchmark-trino-suite.py.
"""
import base64
import hashlib
import json
import math
import os
import random
import re
import ssl
import statistics
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BACKOFF_SECONDS = (0.5, 1.0, 2.0, 5.0)
PROGRESS_SECONDS = 30


class Rejected(Exception):
    """The engine declined to admit the statement. Retried; counted apart."""


FRACTIONAL = re.compile(r"^[-+]?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?$")


def canonical_number(value):
    """A floating-point value to nine significant digits, so that the sum of
    the same numbers in a different order — a parallel or distributed
    aggregate under concurrency — digests alike. Integers are exact."""
    if isinstance(value, bool) or value is None:
        return json.dumps(value)
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return f"{value:.9g}"
    text = str(value)
    if FRACTIONAL.match(text) and ("." in text or "e" in text or "E" in text):
        try:
            return f"{float(text):.9g}"
        except ValueError:
            pass
    return json.dumps(text)


def result_hash(rows):
    """Order-insensitive: rows rendered canonically (scripts/scale-suite.py's
    rendering, floating-point values to nine significant digits) and sorted.
    An ordered statement's rows are sorted too — two runs that break a tie
    in the ORDER BY differently return the same set of rows in a different
    order, and that is the same answer."""
    rendered = sorted("|".join(canonical_number(v) for v in row) for row in rows)
    return hashlib.sha256("\n".join(rendered).encode()).hexdigest()


def percentile(values, fraction):
    """Nearest-rank percentile of a non-empty list."""
    ordered = sorted(values)
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def permutation(ids, client_index):
    """The fixed order client `client_index` runs the statements in: a
    shuffle seeded by the index, so every run of the same client count
    schedules the same sequence on both engines."""
    order = list(ids)
    random.Random(client_index).shuffle(order)
    return order


def statement_sql(statement, engine):
    return statement.get({"kaveon": "kaveon_sql", "trino": "trino_sql"}[engine], statement["sql"])


def statement_windowed(sql):
    """`ORDER BY … LIMIT`: the rows a run returns depend on how a tie at the
    cut is broken, so a different set of the same size is a tie, not a
    wrong answer."""
    upper = sql.upper()
    return "ORDER BY" in upper and "LIMIT" in upper


# ----- engines ---------------------------------------------------------------

class KaveonExecutor:
    def __init__(self, suite):
        sys.path.insert(0, "/app")
        import services.engine_bridge as bridge
        self.bridge = bridge
        self.catalog = suite.get("catalog", "Kaveon")
        self.schema = suite.get("schema", "usage")
        self.timeout = int(os.environ.get("STATEMENT_TIMEOUT_SECONDS", "900"))

    def prepare(self):
        pass

    def execute(self, sql, client_index):
        try:
            # The read path: no result cache, no answer from statistics.
            result = self.bridge.execute(sql, self.catalog, f"throughput-{client_index}", "Admin",
                                         self.schema, timeout=self.timeout,
                                         settings={"result_cache": False, "use_statistics": False})
        except Exception as exc:
            if getattr(exc, "status_code", None) == 429:
                raise Rejected(str(getattr(exc, "detail", exc))) from exc
            raise RuntimeError(str(getattr(exc, "detail", exc))[:300]) from exc
        return result.get("data") or result.get("rows") or []


def validated_next_uri(base_url, candidate):
    base, target = urllib.parse.urlsplit(base_url), urllib.parse.urlsplit(str(candidate or ""))
    if (target.scheme, target.hostname, target.port) != (base.scheme, base.hostname, base.port) \
            or target.username or target.password or target.fragment \
            or not target.path.startswith("/v1/statement/"):
        raise RuntimeError("Trino returned an unsafe next URI")
    return target.geturl()


class TrinoExecutor:
    # Only refusals before execution are rejections; a query Trino kills for
    # memory once it runs is a failed execution, as it is on Kaveon.
    REJECTIONS = {"QUERY_QUEUE_FULL"}

    def __init__(self, suite):
        self.url = os.environ["TRINO_URL"].rstrip("/")
        self.schema = suite["schema"]
        self.ssl = ssl.create_default_context(cafile="/trino-tls/ca.crt")
        password = Path("/trino-auth/client-password").read_text().strip()
        self.auth = "Basic " + base64.b64encode(("qualification:" + password).encode()).decode()
        self.timeout = int(os.environ.get("STATEMENT_TIMEOUT_SECONDS", "900"))

    def _fetch(self, method, url, body=None):
        headers = {"X-Trino-User": "qualification", "X-Trino-Catalog": "opensource",
                   "X-Trino-Schema": self.schema, "Content-Type": "text/plain; charset=utf-8",
                   "Authorization": self.auth}
        request = urllib.request.Request(url, data=body, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, context=self.ssl, timeout=self.timeout) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as exc:
            if exc.code in (429, 503):
                raise Rejected(f"HTTP {exc.code}") from exc
            raise

    def query(self, sql):
        page = self._fetch("POST", self.url + "/v1/statement", sql.encode())
        rows = []
        while True:
            if page.get("error"):
                error = page["error"]
                if error.get("errorName") in self.REJECTIONS:
                    raise Rejected(error.get("errorName"))
                raise RuntimeError(error.get("message", "Trino error"))
            rows.extend(page.get("data") or [])
            if not page.get("nextUri"):
                return rows
            page = self._fetch("GET", validated_next_uri(self.url, page["nextUri"]))

    def prepare(self):
        started = time.time()
        while True:
            try:
                self.query("SELECT 1")
                break
            except Exception as exc:
                if time.time() - started > 300:
                    raise RuntimeError(f"Trino did not become ready: {exc}") from exc
                time.sleep(5)
        tables = json.load(open(os.environ["TRINO_TABLES"], encoding="utf-8"))
        root = f"abfs://opensource@{os.environ['TRINO_ACCOUNT']}.dfs.core.windows.net"
        for table in tables:
            self.query(f"CREATE SCHEMA IF NOT EXISTS opensource.{table['schema']} "
                       f"WITH (location = '{root}/{table['directory']}')")
            columns = ", ".join(f"{name} {kind}" for name, kind in table["columns"])
            self.query(f"CREATE TABLE IF NOT EXISTS opensource.{table['schema']}.{table['name']} ({columns}) "
                       f"WITH (external_location = '{root}/{table['directory']}', format = 'PARQUET')")

    def execute(self, sql, client_index):
        return self.query(sql)


# ----- bookkeeping -----------------------------------------------------------

def _counter():
    return {"executions": 0, "failures": 0, "rejections": 0, "ties": 0}


class Ledger:
    """Thread-safe record of every attempt. The first digest seen for a
    statement (warm-up included) is the reference every later execution of
    it must match."""

    def __init__(self, statement_ids, clients):
        self.lock = threading.Lock()
        self.reference = {}
        self.reference_rows = {}
        self.samples = {}
        self.statements = {sid: {**_counter(), "seconds": [], "errors": [], "attempts": 0} for sid in statement_ids}
        self.clients = [_counter() for _ in range(clients)]
        self.warmup = _counter()

    def _count(self, sid, client_index, phase, key):
        if phase == "warmup":
            self.warmup[key] += 1
            return
        self.statements[sid][key] += 1
        self.clients[client_index][key] += 1

    def rejected(self, sid, client_index, phase):
        with self.lock:
            self._count(sid, client_index, phase, "rejections")

    def failed(self, sid, client_index, phase, kind, message, seconds):
        with self.lock:
            self.statements[sid]["attempts"] += phase != "warmup"
            self._count(sid, client_index, phase, "failures")
            errors = self.statements[sid]["errors"]
            if len(errors) < 3:
                errors.append({"kind": kind, "client": client_index, "phase": phase,
                               "seconds": round(seconds, 3), "message": message[:200]})

    def completed(self, sid, client_index, phase, seconds, rows, windowed):
        """Digest the result and check it against the statement's reference.
        Returns True when the execution was exact. A different set of rows
        of the same size from an `ORDER BY … LIMIT` statement is a tie at
        the cut: counted as an execution and as a tie, not as a failure."""
        digest = result_hash(rows)
        with self.lock:
            reference = self.reference.setdefault(sid, digest)
            reference_rows = self.reference_rows.setdefault(sid, len(rows))
            if sid not in self.samples:
                self.samples[sid] = {"rows": len(rows), "sample": [[str(cell)[:80] for cell in row] for row in rows[:3]]}
        tie = digest != reference and windowed and len(rows) == reference_rows
        if digest != reference and not tie:
            self.failed(sid, client_index, phase, "mismatch",
                        f"digest {digest[:12]} differs from the first-seen {reference[:12]} ({len(rows)} rows, reference {reference_rows})", seconds)
            return False
        with self.lock:
            self.statements[sid]["attempts"] += phase != "warmup"
            self._count(sid, client_index, phase, "executions")
            if tie:
                self._count(sid, client_index, phase, "ties")
            if phase != "warmup":
                self.statements[sid]["seconds"].append(round(seconds, 3))
        return True

    def totals(self):
        with self.lock:
            return {key: sum(c[key] for c in self.clients) for key in _counter()}


def run_client(client_index, statements, engine, execute, ledger, should_stop, phase,
               clock=time.monotonic, sleep=time.sleep):
    """One client's loop: the permutation, repeated, until `should_stop`
    (called with the number of completed passes) says so. A rejection
    retries the same statement after a backoff; any other error moves on."""
    by_id = {s["id"]: s for s in statements}
    order = permutation([s["id"] for s in statements], client_index)
    passes = 0
    while not should_stop(passes):
        for sid in order:
            if should_stop(passes):
                return passes
            statement = by_id[sid]
            sql = statement_sql(statement, engine)
            windowed = statement_windowed(sql)
            retries = 0
            while True:
                if should_stop(passes):
                    return passes
                t0 = clock()
                try:
                    rows = execute(sql, client_index)
                except Rejected:
                    ledger.rejected(sid, client_index, phase)
                    sleep(BACKOFF_SECONDS[min(retries, len(BACKOFF_SECONDS) - 1)])
                    retries += 1
                    continue
                except Exception as exc:
                    ledger.failed(sid, client_index, phase, "error", str(exc), clock() - t0)
                    break
                ledger.completed(sid, client_index, phase, clock() - t0, rows, windowed)
                break
        passes += 1
    return passes


def summarise(ledger, clients, elapsed, config):
    """The per-statement summaries and the THROUGHPUT record."""
    per_statement = []
    failed_everywhere = []
    for sid, entry in ledger.statements.items():
        seconds = entry["seconds"]
        summary = {"id": sid, "executions": entry["executions"], "failures": entry["failures"],
                   "rejections": entry["rejections"], "ties": entry["ties"],
                   "p50_seconds": round(statistics.median(seconds), 3) if seconds else None,
                   "p95_seconds": round(percentile(seconds, 0.95), 3) if seconds else None,
                   "max_seconds": max(seconds) if seconds else None,
                   "result_sha256": ledger.reference.get(sid), **ledger.samples.get(sid, {}),
                   "errors": entry["errors"]}
        if entry["attempts"] and not entry["executions"]:
            failed_everywhere.append(sid)
        per_statement.append(summary)
    totals = ledger.totals()
    successful = totals["executions"]
    return {
        **config,
        "clients": clients,
        "elapsed_seconds": round(elapsed, 3),
        "successful": successful,
        "failures": totals["failures"],
        "rejections": totals["rejections"],
        "ties": totals["ties"],
        "executions_per_second": round(successful / elapsed, 4) if elapsed > 0 else None,
        "per_client": [dict(c) for c in ledger.clients],
        "warmup": dict(ledger.warmup),
        "failed_everywhere": failed_everywhere,
        "statements": per_statement,
    }


def main():
    engine = os.environ["ENGINE"]
    suite = json.load(open(os.environ["SUITE"], encoding="utf-8"))
    clients = int(os.environ["CLIENTS"])
    duration = float(os.environ["DURATION_SECONDS"]) if os.environ.get("DURATION_SECONDS") else None
    rounds = int(os.environ["ROUNDS"]) if os.environ.get("ROUNDS") else None
    if duration is None and rounds is None:
        raise SystemExit("DURATION_SECONDS or ROUNDS is required")
    warmup = float(os.environ.get("WARMUP_SECONDS", "30"))
    statements = suite["statements"]
    executor = {"kaveon": KaveonExecutor, "trino": TrinoExecutor}[engine](suite)
    executor.prepare()
    ledger = Ledger([s["id"] for s in statements], clients)
    window = {}
    barrier = threading.Barrier(clients, action=lambda: window.setdefault("start", time.monotonic()))

    def measured_stop(passes):
        if rounds is not None and passes >= rounds:
            return True
        return duration is not None and time.monotonic() - window["start"] >= duration

    def client(index):
        if warmup > 0:
            warm_until = time.monotonic() + warmup
            run_client(index, statements, engine, executor.execute, ledger,
                       lambda passes: time.monotonic() >= warm_until, "warmup")
        barrier.wait()
        run_client(index, statements, engine, executor.execute, ledger, measured_stop, "measured")

    threads = [threading.Thread(target=client, args=(i,), name=f"client-{i}", daemon=True) for i in range(clients)]
    for thread in threads:
        thread.start()
    done = threading.Event()

    def wait_all():
        for thread in threads:
            thread.join()
        done.set()

    threading.Thread(target=wait_all, daemon=True).start()
    while not done.wait(PROGRESS_SECONDS):
        started = window.get("start")
        print(json.dumps({"progress": {"phase": "measured" if started else "warmup",
                                       "elapsed_seconds": round(time.monotonic() - started, 1) if started else None,
                                       **ledger.totals()}}), flush=True)
    elapsed = time.monotonic() - window["start"]
    config = {"engine": engine, "engine_digest": os.environ.get("ENGINE_DIGEST"), "suite": suite.get("name"),
              "duration_seconds": duration, "rounds": rounds, "warmup_seconds": warmup}
    record = summarise(ledger, clients, elapsed, config)
    for summary in record["statements"]:
        print(json.dumps(summary), flush=True)
    if os.environ.get("OUTPUT"):
        Path(os.environ["OUTPUT"]).write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print("THROUGHPUT=" + json.dumps(record), flush=True)


if __name__ == "__main__":
    main()
