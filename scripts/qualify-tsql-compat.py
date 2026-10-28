"""Run the local distributed T-SQL compatibility corpus.

The script intentionally uses only the standard library so it can run from a
fresh checkout. It records server and client latency, result shapes, and the
first returned rows; it does not print full datasets.
"""

from __future__ import annotations

import argparse
import json
import statistics
import time
import urllib.error
import urllib.request
from pathlib import Path


CASES = [
    {
        "id": "TSQL-004",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(day, 1, DATE '2024-01-31') AS d, "
            "DATEPART(year, DATE '2024-01-31') AS y, "
            "DATEDIFF(day, DATE '2024-01-31', DATE '2024-02-02') AS dd, "
            "CONVERT(INT, '42') AS n, TRY_CONVERT(INT, 'bad') AS bad"
        ),
        "expected": [[19754, 2024, 2, 42, None]],
    },
    {
        "id": "TSQL-005",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(month, 1, DATE '2024-01-31') AS d, "
            "DATEPART(day, DATE '2024-02-29') AS daypart, "
            "CONVERT(DATE, '2024-02-29', 101) AS converted"
        ),
        "expected": [[19782, 29, "2024-02-29"]],
    },
    {
        "id": "TSQL-010",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT borough, DATEPART(year, DATE '2024-01-01') AS y, "
            "ROW_NUMBER() OVER (ORDER BY trips DESC) AS rn "
            "FROM public.nyc_taxi_borough LIMIT 5"
        ),
        "expected_rows": 5,
    },
    {
        "id": "TSQL-011",
        "catalog": "OpenSource",
        "schema": "climate_energy",
        "query": (
            "SELECT a.year, DATEPART(year, DATE '2024-01-01') AS y, COUNT(*) AS c "
            "FROM climate_energy.energy_annual a "
            "JOIN climate_energy.energy_annual b ON a.year = b.year "
            "WHERE DATEADD(day, 1, DATE '2024-01-01') > DATE '2024-01-01' "
            "GROUP BY a.year HAVING COUNT(*) > 0 ORDER BY c DESC LIMIT 5"
        ),
        "expected_rows": 5,
    },
    {
        "id": "TSQL-012",
        "catalog": "OpenSource",
        "schema": "climate_energy",
        "query": (
            "SELECT TOP (2) [year] FROM climate_energy.energy_annual "
            "ORDER BY [year]"
        ),
        "expected_rows": 2,
    },
    {
        "id": "TSQL-013",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(hour, 1, TIMESTAMP '2024-01-31 23:00:00') AS ts, "
            "DATEDIFF(hour, TIMESTAMP '2024-01-31 23:00:00', "
            "TIMESTAMP '2024-02-01 01:00:00') AS h"
        ),
        "expected": [["2024-02-01T00:00:00.000000", 2]],
    },
    {
        "id": "TSQL-014",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT YEAR(DATE '2024-02-29') AS y, "
            "MONTH(DATE '2024-02-29') AS m, DAY(DATE '2024-02-29') AS d"
        ),
        "expected": [[2024, 2, 29]],
    },
    {
        "id": "TSQL-015",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT GETDATE() AS local_now, GETUTCDATE() AS utc_now",
        "expected_rows": 1,
    },
    {
        "id": "TSQL-016",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT borough, trips, ROW_NUMBER() OVER (ORDER BY trips DESC) AS rn "
            "FROM public.nyc_taxi_borough QUALIFY rn <= 2"
        ),
        "expected_rows": 2,
    },
    {
        "id": "TSQL-017",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(year, 1, DATE '2024-02-29') AS y, "
            "DATEADD(quarter, 1, DATE '2024-01-31') AS q, "
            "DATEADD(week, 1, DATE '2024-01-31') AS w, "
            "DATEADD(minute, 1, TIMESTAMP '2024-01-31 23:59:00') AS m, "
            "DATEADD(second, 1, TIMESTAMP '2024-01-31 23:59:59') AS s"
        ),
        "expected": [[20147, 19843, 19760, "2024-02-01T00:00:00.000000", "2024-02-01T00:00:00.000000"]],
    },
    {
        "id": "TSQL-018",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT CONVERT(BIGINT, '9007199254740991') AS b, "
            "CONVERT(FLOAT, '3.5') AS f, CONVERT(VARCHAR, 42) AS v, "
            "TRY_CONVERT(DATE, 'not-a-date') AS bad"
        ),
        "expected": [[9007199254740991, 3.5, "42", None]],
    },
    {
        "id": "TSQL-019",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(day, CAST(NULL AS INT), DATE '2024-01-01') AS d, "
            "CONVERT(INT, CAST(NULL AS VARCHAR)) AS c"
        ),
        "expected": [[None, None]],
    },
    {
        "id": "TSQL-020",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT s.trips, x.tag "
            "FROM public.nyc_taxi_borough s "
            "CROSS APPLY (SELECT 'ok' AS tag) AS x LIMIT 2"
        ),
        "expected_rows": 2,
    },
    {
        "id": "TSQL-021",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT s.trips, x.n "
            "FROM public.nyc_taxi_borough s "
            "CROSS APPLY (VALUES (1),(2)) AS x(n) LIMIT 4"
        ),
        "expected": [[8420, 1], [8420, 2], [17852, 1], [17852, 2]],
    },
    {
        "id": "TSQL-022",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(day, 1, DATE '2024-01-01') AS d, "
            "COUNT(*) AS rows, SUM(actions) AS actions "
            "FROM public.kaveon_events_dashboard"
        ),
        "expected": [[19724, 10120000, 196581091]],
    },
    {
        "id": "TSQL-023",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEPART(year, DATE '2024-01-01') AS y, region, "
            "SUM(actions) AS actions "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY region ORDER BY actions DESC LIMIT 3"
        ),
        "expected": [
            [2024, "Europe", 50724573],
            [2024, "Asia", 44375420],
            [2024, "North America", 38076185],
        ],
    },
    {
        "id": "TSQL-024",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(day, -1, DATE '2024-03-01') AS leap_day, "
            "DATEADD(month, -1, DATE '2024-03-31') AS month_end"
        ),
        "expected": [[19782, 19782]],
    },
    {
        "id": "TSQL-025",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT CONVERT(VARCHAR, CAST(NULL AS INT)) AS converted_null, "
            "TRY_CONVERT(DATE, CAST(NULL AS VARCHAR)) AS date_null"
        ),
        "expected": [[None, None]],
    },
    {
        "id": "TSQL-026",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEPART(quarter, DATE '2024-02-29') AS q, "
            "DATEDIFF(month, DATE '2024-01-31', DATE '2024-03-01') AS m, "
            "DATEDIFF(year, DATE '2020-02-29', DATE '2024-03-01') AS y"
        ),
        "expected": [[1, 1, 4]],
    },
    {
        "id": "TSQL-027",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT DATEADD(day, 1, CAST(NULL AS DATE)) AS d, "
            "TRY_CONVERT(DATETIME2, 'bad') AS bad_ts"
        ),
        "expected": [[None, None]],
    },
    {
        "id": "TSQL-028",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT DATEADD(day, CAST(2 AS BIGINT), DATE '2024-01-01') AS big_delta",
        "expected": [[19725]],
    },
    {
        "id": "TSQL-029",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT CONVERT(DATE, '12/31/2024', 101) AS us_date",
        "expected": [["2024-12-31"]],
    },
    {
        "id": "TSQL-030",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT CONVERT(DATE, '12/31/24', 0) AS us_date",
        "expected": [["2024-12-31"]],
    },
    {
        "id": "TSQL-031",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT CONVERT(DATE, 'Dec 31 2024', 0) AS us_date",
        "expected": [["2024-12-31"]],
    },
    {
        "id": "TSQL-032",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT TRY_CONVERT(DATE, 'Feb 30 2024', 0) AS invalid_date",
        "expected": [[None]],
    },
    {
        "id": "TSQL-033",
        "catalog": "OpenSource",
        "schema": "public",
        "query": "SELECT CONVERT(DATETIME2, '2024-12-31 12:34:56') AS ts",
        "expected": [["2024-12-31T12:34:56.000000"]],
    },
    {
        "id": "TSQL-034",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT platform, SUM(actions) AS actions, COUNT(*) AS rows "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY platform ORDER BY actions DESC LIMIT 5"
        ),
        "expected": [
            ["Desktop", 68124732, 3795230],
            ["Web", 66097142, 3162500],
            ["Mobile", 62359217, 3162270],
        ],
    },
    {
        "id": "TSQL-035",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT region, platform, SUM(actions) AS actions "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY region, platform ORDER BY actions DESC LIMIT 5"
        ),
        "expected": [
            ["Europe", "Desktop", 17586417],
            ["Europe", "Web", 17044891],
            ["Europe", "Mobile", 16093265],
            ["Asia", "Desktop", 15377812],
            ["Asia", "Web", 14930020],
        ],
    },
    {
        "id": "TSQL-036",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT platform, SUM(actions) AS actions "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY platform HAVING SUM(actions) > 60000000 "
            "ORDER BY actions DESC"
        ),
        "expected": [
            ["Desktop", 68124732],
            ["Web", 66097142],
            ["Mobile", 62359217],
        ],
    },
    {
        "id": "TSQL-037",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT COUNT(DISTINCT user_id) AS users, "
            "COUNT(DISTINCT region) AS regions "
            "FROM public.kaveon_events_dashboard"
        ),
        "expected": [[44000, 6]],
    },
    {
        "id": "TSQL-038",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT region, COUNT(DISTINCT user_id) AS users "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY region ORDER BY users DESC"
        ),
        "expected": [
            ["Europe", 11353],
            ["Asia", 9933],
            ["North America", 8521],
            ["South America", 5679],
            ["Africa", 5679],
            ["Oceania", 2835],
        ],
    },
    {
        "id": "TSQL-039",
        "catalog": "OpenSource",
        "schema": "public",
        "query": (
            "SELECT region, COUNT(DISTINCT user_id) AS users "
            "FROM public.kaveon_events_dashboard "
            "GROUP BY region HAVING COUNT(DISTINCT user_id) > 8000 "
            "ORDER BY users DESC"
        ),
        "expected": [
            ["Europe", 11353],
            ["Asia", 9933],
            ["North America", 8521],
        ],
    },
]


def execute(endpoint: str, case: dict) -> tuple[dict, float]:
    payload = json.dumps(
        {
            "query": case["query"],
            "catalog": case["catalog"],
            "schema": case["schema"],
            "result_delivery": "inline",
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        endpoint,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=120) as response:
        result = json.load(response)
    return result, (time.perf_counter() - started) * 1000


def validate(case: dict, result: dict) -> None:
    if result.get("state") != "FINISHED":
        raise AssertionError(f"{case['id']} did not finish: {result}")
    rows = result.get("data") or []
    if "expected" in case and rows != case["expected"]:
        raise AssertionError(f"{case['id']} result mismatch: {rows!r}")
    if "expected_rows" in case and len(rows) != case["expected_rows"]:
        raise AssertionError(f"{case['id']} expected {case['expected_rows']} rows, got {len(rows)}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--endpoint", default="http://127.0.0.1:8081/v1/statement")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("--repetitions must be positive")

    report = {
        "endpoint": args.endpoint,
        "repetitions": args.repetitions,
        "cases": [],
    }
    for case in CASES:
        timings: list[float] = []
        last: dict | None = None
        for _ in range(args.repetitions):
            try:
                last, elapsed = execute(args.endpoint, case)
            except (urllib.error.URLError, TimeoutError) as error:
                raise SystemExit(f"{case['id']} request failed: {error}") from error
            validate(case, last)
            timings.append(elapsed)
        report["cases"].append(
            {
                "id": case["id"],
                "rows": len(last.get("data") or []),
                "columns": [column.get("name") for column in last.get("columns", [])],
                "server_elapsed_ms": last.get("elapsed_ms"),
                "client_ms": {
                    "min": round(min(timings), 2),
                    "median": round(statistics.median(timings), 2),
                    "max": round(max(timings), 2),
                },
            }
        )

    encoded = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
