"""Generate TPC-H at a scale factor as Delta tables (Parquet files under a
Delta log) in the `opensource` container through Trino's tpch connector,
for Tier 3 of the benchmark program.

Runs inside the cluster during a Trino window with the chart's
`trino.tpch.enabled=true` (infra/aks/tpch-generate-job.yaml). For each of
the eight tables: CREATE TABLE AS SELECT from tpch.sf<N> into a Delta
table under <TPCH_ROOT>/<table>/ through the chart's `lake` Delta catalog,
then COUNT(*) and DESCRIBE, so the manifest the two engines register from
carries the exact row counts and column types. Delta rather than a plain
Parquet directory because that is the multi-file table both engines read
from object storage today: Trino through its Delta connector, Kaveon
through its Delta log reader (a directory of Parquet files with no log is
not a table for Kaveon yet). Prints one JSON line per table and a final
TPCH_GENERATE line; the manifest is written to TPCH_OUTPUT.

Environment: TRINO_URL, /trino-tls/ca.crt, /trino-auth/client-password,
TRINO_ACCOUNT, TPCH_SCALE (default 100), TPCH_ROOT (container-relative,
default benchmarks/tpch/sf<N>), TPCH_OUTPUT.
"""
import base64
import json
import os
import ssl
import time
import urllib.parse
import urllib.request
from pathlib import Path

TABLES = ["region", "nation", "supplier", "part", "partsupp", "customer", "orders", "lineitem"]


def validated_next_uri(base_url, candidate):
    base = urllib.parse.urlparse(base_url)
    parsed = urllib.parse.urlparse(candidate)
    if (parsed.scheme, parsed.netloc) != (base.scheme, base.netloc):
        raise RuntimeError("Trino nextUri left the configured host")
    return candidate


class Trino:
    def __init__(self):
        self.url = os.environ["TRINO_URL"].rstrip("/")
        self.ssl = ssl.create_default_context(cafile="/trino-tls/ca.crt")
        password = Path("/trino-auth/client-password").read_text().strip()
        self.auth = "Basic " + base64.b64encode(("qualification:" + password).encode()).decode()

    def _fetch(self, method, url, body=None):
        headers = {"X-Trino-User": "qualification", "Content-Type": "text/plain; charset=utf-8",
                   "Authorization": self.auth}
        request = urllib.request.Request(url, data=body, method=method, headers=headers)
        with urllib.request.urlopen(request, context=self.ssl, timeout=900) as response:
            return json.loads(response.read().decode())

    def query(self, sql):
        page = self._fetch("POST", self.url + "/v1/statement", sql.encode())
        rows, columns = [], None
        while True:
            if page.get("error"):
                raise RuntimeError(page["error"].get("message", "Trino error"))
            columns = columns or page.get("columns")
            rows.extend(page.get("data") or [])
            if not page.get("nextUri"):
                return rows, columns
            page = self._fetch("GET", validated_next_uri(self.url, page["nextUri"]))


def wait_ready(trino, attempts=60):
    for attempt in range(attempts):
        try:
            trino.query("SELECT 1")
            return
        except Exception:
            time.sleep(10)
    raise RuntimeError("Trino did not become ready")


def main():
    scale = int(os.environ.get("TPCH_SCALE", "100"))
    account = os.environ["TRINO_ACCOUNT"]
    directory = os.environ.get("TPCH_ROOT", f"benchmarks/tpch/sf{scale}").strip("/")
    root = f"abfs://opensource@{account}.dfs.core.windows.net/{directory}"
    schema = f"tpch_sf{scale}"
    trino = Trino()
    wait_ready(trino)
    trino.query(f"CREATE SCHEMA IF NOT EXISTS lake.{schema} WITH (location = '{root}')")
    manifest = []
    for table in TABLES:
        started = time.time()
        existing, _ = trino.query(
            f"SELECT count(*) FROM lake.information_schema.tables "
            f"WHERE table_schema = '{schema}' AND table_name = '{table}'")
        if existing[0][0] == 0:
            trino.query(
                f"CREATE TABLE lake.{schema}.{table} "
                f"WITH (location = '{root}/{table}') "
                f"AS SELECT * FROM tpch.sf{scale}.{table}")
        rows, _ = trino.query(f"SELECT count(*) FROM lake.{schema}.{table}")
        described, _ = trino.query(
            f"SELECT column_name, data_type FROM lake.information_schema.columns "
            f"WHERE table_schema = '{schema}' AND table_name = '{table}' ORDER BY ordinal_position")
        columns = [[name, kind] for name, kind in described]
        record = {"schema": schema, "name": table, "format": "delta", "directory": f"{directory}/{table}/",
                  "columns": columns, "rows": rows[0][0], "seconds": round(time.time() - started, 1)}
        manifest.append(record)
        print(json.dumps(record), flush=True)
    output = os.environ.get("TPCH_OUTPUT")
    if output:
        Path(output).write_text(json.dumps(manifest, indent=1), encoding="utf-8")
    print("TPCH_GENERATE=" + json.dumps({"scale": scale, "tables": manifest}), flush=True)


if __name__ == "__main__":
    main()
