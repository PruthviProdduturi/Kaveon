"""A suite JSON on Trino, alone on the worker nodes, through the read-only
`opensource` Hive catalog of `infra/helm/kaveon-trino-benchmark`.

The counterpart of scripts/scale-suite.py: the same statements, one warm-up
then three timed executions, one JSON record per statement with the median,
the row count and the engine-independent result digest (identical rendering
to scale-suite.py, so the two records compare directly), then a final
TRINO_SUITE line. Runs inside the cluster during a leased window.

Environment: TRINO_URL, /trino-tls/ca.crt, /trino-auth/client-password,
SUITE (suite JSON), TRINO_TABLES (a JSON file listing the external tables to
declare: schema, name, directory under the container root, columns as
Trino types), TRINO_ACCOUNT (storage account), TRINO_OUTPUT.
"""
import base64
import hashlib
import json
import os
import ssl
import statistics
import time
import urllib.parse
import urllib.request
from pathlib import Path

REPETITIONS = 3


def validated_next_uri(base_url, candidate):
    base, target = urllib.parse.urlsplit(base_url), urllib.parse.urlsplit(str(candidate or ""))
    if (target.scheme, target.hostname, target.port) != (base.scheme, base.hostname, base.port) \
            or target.username or target.password or target.fragment \
            or not target.path.startswith("/v1/statement/"):
        raise RuntimeError("Trino returned an unsafe next URI")
    return target.geturl()


class Trino:
    def __init__(self, schema, catalog="opensource"):
        self.catalog = catalog
        self.url = os.environ["TRINO_URL"].rstrip("/")
        self.schema = schema
        self.ssl = ssl.create_default_context(cafile="/trino-tls/ca.crt")
        password = Path("/trino-auth/client-password").read_text().strip()
        self.auth = "Basic " + base64.b64encode(("qualification:" + password).encode()).decode()

    def _fetch(self, method, url, body=None):
        headers = {"X-Trino-User": "qualification", "X-Trino-Catalog": self.catalog,
                   "X-Trino-Schema": self.schema, "Content-Type": "text/plain; charset=utf-8",
                   "Authorization": self.auth}
        request = urllib.request.Request(url, data=body, method=method, headers=headers)
        with urllib.request.urlopen(request, context=self.ssl, timeout=900) as response:
            return json.loads(response.read().decode())

    def query(self, sql):
        page = self._fetch("POST", self.url + "/v1/statement", sql.encode())
        rows = []
        while True:
            if page.get("error"):
                raise RuntimeError(page["error"].get("message", "Trino error"))
            rows.extend(page.get("data") or [])
            if not page.get("nextUri"):
                return rows
            page = self._fetch("GET", validated_next_uri(self.url, page["nextUri"]))


def result_hash(rows, ordered):
    def cell(value):
        if isinstance(value, bool) or value is None:
            return json.dumps(value)
        if isinstance(value, float):
            return f"{value:.6f}" if value != int(value) else str(int(value))
        if isinstance(value, int):
            return str(value)
        return json.dumps(str(value))
    rendered = ["|".join(cell(v) for v in row) for row in rows]
    if not ordered:
        rendered.sort()
    return hashlib.sha256("\n".join(rendered).encode()).hexdigest()


def wait_ready(trino, timeout=300):
    """A freshly activated coordinator answers 500 until its authenticators
    and workers are up; wait for the first trivial statement to succeed."""
    started = time.time()
    while True:
        try:
            trino.query("SELECT 1")
            return
        except Exception as exc:
            if time.time() - started > timeout:
                raise RuntimeError(f"Trino did not become ready: {exc}") from exc
            time.sleep(5)


def declare_tables(trino, tables, account):
    root = f"abfs://opensource@{account}.dfs.core.windows.net"
    for table in tables:
        if table.get("format") == "delta":
            # A Delta table is registered by its log through the `lake`
            # Delta catalog (delta.register-table-procedure.enabled).
            trino.query(f"CREATE SCHEMA IF NOT EXISTS lake.{table['schema']}")
            existing, _ = trino.query(
                f"SELECT count(*) FROM lake.information_schema.tables "
                f"WHERE table_schema = '{table['schema']}' AND table_name = '{table['name']}'")
            if existing[0][0] == 0:
                trino.query(f"CALL lake.system.register_table(schema_name => '{table['schema']}', "
                            f"table_name => '{table['name']}', "
                            f"table_location => '{root}/{table['directory'].rstrip('/')}')")
            continue
        trino.query(f"CREATE SCHEMA IF NOT EXISTS opensource.{table['schema']} "
                    f"WITH (location = '{root}/{table['directory']}')")
        columns = ", ".join(f"{name} {kind}" for name, kind in table["columns"])
        trino.query(f"CREATE TABLE IF NOT EXISTS opensource.{table['schema']}.{table['name']} ({columns}) "
                    f"WITH (external_location = '{root}/{table['directory']}', format = 'PARQUET')")


def main():
    suite = json.load(open(os.environ["SUITE"], encoding="utf-8"))
    tables = json.load(open(os.environ["TRINO_TABLES"], encoding="utf-8"))
    trino = Trino(suite["schema"], suite.get("trino_catalog", "opensource"))
    wait_ready(trino)
    declare_tables(trino, tables, os.environ["TRINO_ACCOUNT"])
    records = []
    for statement in suite["statements"]:
        sql = statement.get("trino_sql", statement["sql"])
        record = {"id": statement["id"], "seconds": [], "rows": None, "error": None,
                  "adapted": "trino_sql" in statement}
        try:
            trino.query(sql)
            for _ in range(REPETITIONS):
                t0 = time.time()
                rows = trino.query(sql)
                record["seconds"].append(round(time.time() - t0, 3))
            record["rows"] = len(rows)
            record["sample"] = [[str(cell)[:80] for cell in row] for row in rows[:3]]
            record["result_sha256"] = result_hash(rows, statement.get("ordered", "ORDER BY" in sql.upper()))
            record["median_seconds"] = round(statistics.median(record["seconds"]), 3)
        except Exception as exc:
            record["error"] = str(exc)[:300]
        records.append(record)
        print(json.dumps(record), flush=True)
    measured = [r for r in records if r.get("median_seconds") is not None]
    output = {"suite": suite.get("name"), "repetitions": REPETITIONS, "statements": len(records),
              "measured": len(measured), "records": records}
    Path(os.environ["TRINO_OUTPUT"]).write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    print("TRINO_SUITE=" + json.dumps({k: v for k, v in output.items() if k != "records"}), flush=True)


if __name__ == "__main__":
    main()
