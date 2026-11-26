"""Compare the frozen read-only SQL corpus through the stock Trino Python client."""
import argparse
from contextlib import closing
from datetime import date, datetime
from decimal import Decimal
import json
from pathlib import Path
from urllib.parse import urlsplit

import requests
import trino
from trino.dbapi import connect
from trino.exceptions import TrinoQueryError


FIXTURE = Path(__file__).parent / "fixtures" / "trino-client-contract.json"


def endpoint(value):
    url = urlsplit(value)
    if url.scheme not in {"http", "https"} or not url.hostname or url.path not in {"", "/"}:
        raise argparse.ArgumentTypeError("Use the HTTP(S) origin of a coordinator.")
    if url.username or url.password or url.query or url.fragment:
        raise argparse.ArgumentTypeError("The origin must contain only a host and optional port.")
    return url


def address(origin):
    port = origin.port or (443 if origin.scheme == "https" else 80)
    return origin.scheme, origin.hostname, port


def run(origin, sql, *, kaveon):
    scheme, host, port = address(origin)
    connection = connect(
        host=host, port=port, http_scheme=scheme, user="qualification",
        source="trino-client-contract", request_timeout=30,
        session_properties={"result_cache": "false"} if kaveon else {},
    )
    with closing(connection) as conn:
        with closing(conn.cursor()) as cursor:
            try:
                cursor.execute(sql)
                rows = cursor.fetchall()
            except TrinoQueryError as exc:
                return {"error": {"name": exc.error_name, "code": exc.error_code, "type": exc.error_type}}
            # DB-API description omits the protocol's structured type arguments.
            # Read the same response the pinned client used to decode the rows.
            columns = cursor._query.columns
            return {"rows": rows, "columns": [
                {key: col[key] for key in ("name", "type", "typeSignature")} for col in columns
            ]}


def encode(value):
    if isinstance(value, Decimal):
        return {"decimal": str(value)}
    if isinstance(value, (datetime, date)):
        return {"temporal": value.isoformat()}
    raise TypeError(type(value).__name__)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kaveon", required=True, type=endpoint)
    parser.add_argument("--trino", required=True, type=endpoint)
    parser.add_argument("--reference-version", default="483")
    parser.add_argument("--output", type=Path, help="Write the observations to this local file.")
    args = parser.parse_args()
    if address(args.kaveon) == address(args.trino):
        parser.error("Two distinct coordinators are required.")
    version = requests.get(args.trino.geturl().rstrip("/") + "/v1/info", timeout=10)
    version.raise_for_status()
    reference = version.json()["nodeVersion"]["version"]
    if reference != args.reference_version:
        parser.error(f"Expected reference version {args.reference_version}, found {reference}.")
    cases = json.loads(FIXTURE.read_text(encoding="utf-8"))
    report = {"reference_version": reference, "client_version": trino.__version__, "cases": {}}
    for case in cases:
        ref = run(args.trino, case["sql"], kaveon=False)
        actual = run(args.kaveon, case["sql"], kaveon=True)
        valid = (ref.get("error") == case.get("expected_error"))
        exact = valid and actual == ref
        report["cases"][case["name"]] = {"sql": case["sql"], "reference": ref, "kaveon": actual, "exact": exact}
        print(f"{'PASS' if exact else 'FAIL'}  {case['name']}")
    if args.output:
        args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False, default=encode) + "\n", encoding="utf-8")
    passed = sum(row["exact"] for row in report["cases"].values())
    print(f"{passed}/{len(cases)} exact against Trino {reference}")
    return 0 if passed == len(cases) else 1


if __name__ == "__main__":
    raise SystemExit(main())
