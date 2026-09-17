"""Differential cases: the same statements against two tables that hold the
same rows in different Parquet encodings (dictionary-schema and plain), so
any divergence is an Engine defect. Runs inside the cluster through the
bridge; prints one JSON line per case with both timings and whether the
result sets match, then a summary line. Results are compared as sorted
multisets of stringified rows so ordering-free statements compare fairly;
ORDER BY statements compare in order."""
import json
import os
import sys
import time

sys.path.insert(0, "/app")
import services.engine_bridge as eb  # noqa: E402

A = os.environ.get("TABLE_A", "kaveon_product.kaveon_events_enriched")
B = os.environ.get("TABLE_B", "kaveon_product.kaveon_events_plain")
U = "kaveon_product.kaveon_events_users"

CASES = [
    ("distinct_values", "SELECT DISTINCT region FROM {T} ORDER BY region", True),
    ("order_by_dict_desc", "SELECT country, region FROM {T} WHERE surface = 'Export' AND event_date = '2026-07-20' ORDER BY country DESC, region LIMIT 5", True),
    ("in_list", "SELECT country, SUM(actions) AS a FROM {T} WHERE country IN ('Japan', 'Brazil', 'Kenya') GROUP BY country ORDER BY country", True),
    ("not_equal", "SELECT COUNT(*) AS n FROM {T} WHERE region <> 'Asia'", False),
    ("like_prefix", "SELECT COUNT(*) AS n FROM {T} WHERE country LIKE 'United%'", False),
    ("or_predicate", "SELECT COUNT(*) AS n FROM {T} WHERE region = 'Africa' OR platform = 'Mobile'", False),
    ("null_safe", "SELECT COUNT(*) AS n FROM {T} WHERE country IS NOT NULL AND industry IS NULL", False),
    ("case_expr", "SELECT CASE WHEN region = 'Europe' THEN 'EU' ELSE 'Other' END AS zone, SUM(sessions) AS s FROM {T} GROUP BY CASE WHEN region = 'Europe' THEN 'EU' ELSE 'Other' END ORDER BY zone", True),
    ("upper_fn", "SELECT UPPER(surface) AS s, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-04' GROUP BY UPPER(surface) ORDER BY s", True),
    ("avg_min_max", "SELECT platform, AVG(duration_sec) AS d, MIN(latency_p75_ms) AS lo, MAX(latency_p75_ms) AS hi FROM {T} GROUP BY platform ORDER BY platform", True),
    ("text_min_max_grouped", "SELECT surface, MIN(country) AS first_country, MAX(event_date) AS last_day FROM {T} GROUP BY surface ORDER BY surface", True),
    ("three_keys", "SELECT region, platform, license, SUM(actions) AS a FROM {T} GROUP BY region, platform, license ORDER BY a DESC LIMIT 12", True),
    ("mixed_key_types", "SELECT country, user_id % 7 AS bucket, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-15' AND surface = 'Chat' GROUP BY country, user_id % 7 ORDER BY n DESC, country, bucket LIMIT 10", True),
    ("having", "SELECT industry, SUM(errors) AS e FROM {T} GROUP BY industry HAVING SUM(errors) > 0 ORDER BY e DESC LIMIT 5", True),
    ("count_distinct_lowcard", "SELECT COUNT(DISTINCT country) AS c, COUNT(DISTINCT surface) AS s FROM {T}", False),
    ("count_distinct_grouped_lowcard", "SELECT region, COUNT(DISTINCT country) AS c FROM {T} GROUP BY region ORDER BY region", True),
    ("date_range_and_dim", "SELECT country, SUM(queries_run) AS q FROM {T} WHERE event_date BETWEEN '2026-07-10' AND '2026-07-12' AND platform = 'Web' GROUP BY country ORDER BY q DESC LIMIT 5", True),
    ("join_users", "SELECT u.locale, SUM(t.actions) AS a FROM {T} t JOIN {U} u ON u.user_id = t.user_id WHERE t.event_date = '2026-07-04' AND t.surface = 'API' GROUP BY u.locale ORDER BY a DESC LIMIT 5", True),
    ("join_dict_keys", "SELECT t.country, u.country AS user_country, COUNT(*) AS n FROM {T} t JOIN {U} u ON u.user_id = t.user_id WHERE t.event_date = '2026-07-04' AND t.surface = 'API' AND t.country <> u.country GROUP BY t.country, u.country ORDER BY n DESC LIMIT 3", True),
    ("topn_by_key", "SELECT country, surface, SUM(actions) AS a FROM {T} WHERE event_date = '2026-07-31' GROUP BY country, surface ORDER BY country, surface LIMIT 8", True),
    ("offset", "SELECT country, SUM(actions) AS a FROM {T} GROUP BY country ORDER BY a DESC LIMIT 5 OFFSET 5", True),
    ("limit_no_order", "SELECT COUNT(*) AS n FROM (SELECT country FROM {T} WHERE event_date = '2026-07-04' LIMIT 1000) x", False),
    ("union_all", "SELECT 'a' AS k, COUNT(*) AS n FROM {T} WHERE region = 'Asia' UNION ALL SELECT 'e', COUNT(*) FROM {T} WHERE region = 'Europe' ORDER BY k", True),
    ("subquery_in", "SELECT COUNT(*) AS n FROM {T} WHERE country IN (SELECT country FROM {U} WHERE locale = 'ja-JP' GROUP BY country)", False),
    ("arith_projection", "SELECT country, SUM(actions * 2 + sessions) AS x FROM {T} WHERE event_date = '2026-07-04' GROUP BY country ORDER BY x DESC LIMIT 3", True),
    ("string_concat", "SELECT country || ' / ' || region AS place, COUNT(*) AS n FROM {T} WHERE event_date = '2026-07-04' AND surface = 'Chat' GROUP BY country || ' / ' || region ORDER BY n DESC LIMIT 3", True),
    ("window_rank", "SELECT country, a, RANK() OVER (ORDER BY a DESC) AS r FROM (SELECT country, SUM(actions) AS a FROM {T} WHERE event_date = '2026-07-04' GROUP BY country) x ORDER BY r LIMIT 3", True),
    ("count_star_filter_only", "SELECT COUNT(*) AS n FROM {T} WHERE surface = 'Chat' AND country = 'India' AND event_date >= '2026-07-20'", False),
]


def run(sql):
    t0 = time.time()
    # Bypass the coordinator's result cache: both timings are the Engine's.
    result = eb.execute(sql, "OpenSource", "differential", "Admin", "kaveon_product", timeout=900,
                        settings={"result_cache": False})
    rows = result.get("data") or result.get("rows") or []
    return round(time.time() - t0, 2), rows


def main():
    same = 0
    records = []
    for name, template, ordered in CASES:
        record = {"case": name}
        for label, table in (("a", A), ("b", B)):
            sql = template.format(T=table, U=U)
            try:
                seconds, rows = run(sql)
                record[label] = {"seconds": seconds, "rows": len(rows), "sample": rows[:3]}
                key = [json.dumps(r, default=str) for r in rows]
                record[label + "_key"] = key if ordered else sorted(key)
            except Exception as exc:
                detail = str(getattr(exc, "detail", exc))
                try:
                    queries = eb._request("GET", "/v1/query", "KAVEON_ENGINE_BRIDGE_TOKEN", "kaveon-system", role="admin") or []
                    failed = [q for q in queries if q.get("state") == "FAILED"]
                    if failed:
                        detail = json.dumps(failed[-1].get("error"))[:300]
                except Exception:
                    pass
                record[label] = {"error": detail[:300]}
        match = "a_key" in record and "b_key" in record and record["a_key"] == record["b_key"]
        record["match"] = match
        same += int(match)
        record.pop("a_key", None); record.pop("b_key", None)
        records.append(record)
        print(json.dumps(record, default=str), flush=True)
    print("DIFFERENTIAL=" + json.dumps({"cases": len(CASES), "match": same, "records": records}, default=str))


if __name__ == "__main__":
    main()
