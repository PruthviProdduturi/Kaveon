"""Auto-curation — a context spec derived from the Engine's statistics.

Each test states a table the way the Engine's record states it and asserts
what the derivation makes of it: which columns become dimensions, measures or
identifiers, what a measure's outlier-safe range is, what the time dimension
and its "latest" are, and that every element says which statistic it came
from. The merge tests are the idempotence contract: a regenerate refreshes
what the record owns and never touches what a curator typed.
"""
import dlm.curation as curation
from dlm.engine import _merge_spec, _suggest_spec


def _statistics(rows, columns, *, depth="full", stale=False, version=None):
    return {
        "table": "OpenSource.public.t", "table_id": "t",
        "stale": stale,
        "source_version": version or {"kind": "listing", "identity_sha256": "abc123", "files": 1},
        "statistics": {
            "rows": rows, "depth": depth, "computed_at_ms": 1790100145262,
            "source_version": version or {"kind": "listing", "identity_sha256": "abc123", "files": 1},
            "columns": columns,
        },
    }


def _column(name, arrow, *, distinct_sketch=True, quantiles=None, null_count=0,
            lo=None, hi=None, distinct_exact=None):
    out = {"name": name, "data_type": arrow, "null_count": null_count, "bounds_exact": True}
    if lo is not None:
        out["min"] = lo
    if hi is not None:
        out["max"] = hi
    if distinct_sketch:
        out["distinct"] = "SwEMA…"
    if distinct_exact is not None:
        out["distinct_exact"] = distinct_exact
    if quantiles:
        out["quantiles"] = "TAHIAE…"
    return out


DATASET_COLUMNS = [
    {"table_name": "t", "column_name": "user_id", "data_type": "bigint", "is_dimension": False},
    {"table_name": "t", "column_name": "country", "data_type": "varchar", "is_dimension": True},
    {"table_name": "t", "column_name": "note", "data_type": "varchar", "is_dimension": True},
    {"table_name": "t", "column_name": "latency_ms", "data_type": "bigint", "is_dimension": False},
    {"table_name": "t", "column_name": "usage_date", "data_type": "varchar", "is_dimension": True},
]
DATASET_METRICS = [
    {"name": "Rows", "expression": "COUNT(*)", "metric_type": "count"},
    {"name": "latency_ms", "expression": "SUM(latency_ms)", "metric_type": "sum"},
    {"name": "distinct user_id", "expression": "COUNT(DISTINCT user_id)", "metric_type": "count_distinct"},
]

PROBE_ROW = {
    # distinct columns, in the order the record lists them
    "user_id": 43101, "country": 26, "note": 3_000_000, "latency_ms": 640, "usage_date": 230,
    # quantile columns
    "latency_ms_q": [68.0, 1518.0, 2958.0],
}


def _facts(**overrides):
    columns = [
        _column("user_id", "Int64", quantiles=True, lo={"Int": 1}, hi={"Int": 44000}),
        _column("country", "Utf8", lo={"Text": "Argentina"}, hi={"Text": "United States"}),
        _column("note", "Utf8", lo={"Text": "a"}, hi={"Text": "zz"}),
        _column("latency_ms", "Int64", quantiles=True, null_count=12,
                lo={"Int": 50}, hi={"Int": 3000}),
        _column("usage_date", "Utf8", lo={"Text": "2026-01-01"}, hi={"Text": "2026-08-18"}),
    ]
    statistics = _statistics(10_000_000, columns, **overrides)
    arrow_types = {c["column_name"]: c["data_type"] for c in DATASET_COLUMNS}

    def run_probe(sql):
        assert "APPROX_COUNT_DISTINCT" in sql
        row = [PROBE_ROW["user_id"], PROBE_ROW["country"], PROBE_ROW["note"],
               PROBE_ROW["latency_ms"], PROBE_ROW["usage_date"],
               [1.0, 1.0, 1.0], PROBE_ROW["latency_ms_q"]]
        return row, {"mode": "context"}

    return curation.facts_from_engine(statistics, overrides.pop("shape", None), arrow_types,
                                      run_probe=run_probe, table="OpenSource.public.t")


# ── the probe ─────────────────────────────────────────────────────────────────

def test_probe_only_names_columns_that_carry_a_sketch():
    columns = [_column("a", "Utf8"), _column("b", "Int64", distinct_sketch=False),
               _column("c", "Int64", quantiles=True)]
    sent = {}

    def run_probe(sql):
        sent["sql"] = sql
        return [3, 5, [1.0, 2.0, 3.0]], {"mode": "context"}

    curation.facts_from_engine(_statistics(10, columns), None,
                               {"a": "varchar", "b": "bigint", "c": "bigint"},
                               run_probe=run_probe, table="cat.sch.t")
    assert "APPROX_COUNT_DISTINCT(a)" in sent["sql"]
    assert "APPROX_COUNT_DISTINCT(b)" not in sent["sql"]     # no sketch on record
    assert "APPROX_PERCENTILE(c, ARRAY[0.01, 0.5, 0.99])" in sent["sql"]


def test_probe_answered_by_a_scan_is_discarded():
    columns = [_column("a", "Utf8")]
    facts = curation.facts_from_engine(_statistics(10, columns), None, {"a": "varchar"},
                                       run_probe=lambda sql: ([7], {"mode": "distributed"}),
                                       table="cat.sch.t")
    assert facts.distinct("a") is None        # a derived spec is never worth a scan


def test_a_stale_record_is_not_probed():
    columns = [_column("a", "Utf8")]
    calls = []
    facts = curation.facts_from_engine(_statistics(10, columns, stale=True), None, {"a": "varchar"},
                                       run_probe=lambda sql: calls.append(sql) or ([7], {"mode": "context"}),
                                       table="cat.sch.t")
    assert calls == []
    assert facts.stale is True


def test_exact_distinct_on_record_beats_the_sketch():
    columns = [_column("a", "Utf8", distinct_exact=4)]
    facts = curation.facts_from_engine(_statistics(10, columns), None, {"a": "varchar"},
                                       run_probe=lambda sql: ([99], {"mode": "context"}),
                                       table="cat.sch.t")
    assert facts.distinct("a") == 4
    assert facts.distinct_basis("a") == "exact count"


# ── roles ─────────────────────────────────────────────────────────────────────

def test_distinct_counts_decide_dimension_measure_identifier():
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    assert "country" in spec["dimensions"]                   # 26 of 10M rows
    assert "note" not in spec["dimensions"]                  # 3M distinct values is free text
    assert spec["identifiers"]["note"]["reason"] == "text"
    assert "user_id" not in spec["dimensions"]               # numeric, 43k of 10M → a measure
    assert "latency_ms" not in spec["dimensions"]            # numeric → a measure


def test_a_near_unique_column_is_an_identifier_not_a_dimension():
    columns = [_column("row_key", "Utf8")]
    facts = curation.facts_from_engine(
        _statistics(1_000, columns), None, {"row_key": "varchar"},
        run_probe=lambda sql: ([1_000], {"mode": "context"}), table="cat.sch.t")
    spec = curation.derive([{"column_name": "row_key", "data_type": "varchar", "is_dimension": True}],
                           [], facts)
    assert "row_key" not in spec["dimensions"]
    assert spec["identifiers"]["row_key"]["reason"] == "identifier"
    assert spec["identifiers"]["row_key"]["evidence"]["statistic"] == "columns[].distinct"


def test_a_declared_shape_wins_over_the_statistics():
    shape = {"dimensions": [{"name": "note"}], "measures": [{"column": "country"}],
             "time": {"column": "usage_date"}}
    columns = [
        _column("note", "Utf8"), _column("country", "Utf8"),
        _column("usage_date", "Utf8", lo={"Text": "2026-01-01"}, hi={"Text": "2026-08-18"}),
    ]
    facts = curation.facts_from_engine(
        _statistics(10_000_000, columns), shape,
        {"note": "varchar", "country": "varchar", "usage_date": "varchar"},
        run_probe=lambda sql: ([3_000_000, 26, 230], {"mode": "context"}), table="cat.sch.t")
    spec = curation.derive(
        [{"column_name": c, "data_type": "varchar", "is_dimension": True}
         for c in ("note", "country", "usage_date")], [], facts)
    # `note` is free text by cardinality but the shape declares it a dimension
    assert "note" in spec["dimensions"]
    assert spec["dimensions"]["note"]["evidence"]["statistic"] == "declared shape"
    assert "country" not in spec["dimensions"]               # declared a measure
    assert spec["time"]["column"] == "usage_date"


def test_indexable_dimensions_follow_the_cardinality():
    facts = _facts()
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, facts, engine=True)
    assert curation.indexable_dimensions(spec) == ["country"]
    assert spec["dimensions"]["country"]["top_n"] == 26
    assert spec["dimensions"]["country"]["cardinality"] == 26


# ── ranges, nulls, evidence ───────────────────────────────────────────────────

def test_quantiles_set_the_outlier_safe_range_and_nulls_set_optionality():
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    measure = spec["metrics"]["latency_ms"]
    assert measure["range"]["min"] == 50 and measure["range"]["max"] == 3000
    assert measure["range"]["p50"] == 1518.0
    # the range a person is shown is p01..p99, not the single worst row
    assert measure["range"]["outlier_safe_max"] == 2958.0
    assert measure["range"]["optional"] is True and measure["range"]["null_count"] == 12
    assert measure["evidence"]["basis"] == "kll k=200"


def test_every_element_records_the_statistic_and_the_version_it_came_from():
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    evidence = spec["dimensions"]["country"]["evidence"]
    assert evidence["statistic"] == "columns[].distinct"
    assert evidence["basis"] == "hyperloglog p=12"
    assert evidence["source_version"]["identity_sha256"] == "abc123"
    assert evidence["computed_at_ms"] == 1790100145262
    assert spec["derived_from"]["source"] == "engine_statistics"
    assert spec["derived_from"]["depth"] == "full"


def test_a_count_distinct_metric_is_non_additive_and_approximate_on_the_engine():
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    assert spec["metrics"]["distinct user_id"]["additive"] is False
    assert spec["metrics"]["distinct user_id"]["approximate"] is True
    assert spec["metrics"]["latency_ms"]["additive"] is True


# ── the time dimension ────────────────────────────────────────────────────────

def test_the_time_dimension_comes_from_the_bounds_and_latest_is_the_data_not_today():
    spec = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    time = spec["time"]
    assert time["column"] == "usage_date"
    assert time["min"] == "2026-01-01" and time["max"] == "2026-08-18"
    assert time["grain"] == "day"
    assert time["latest"] == "2026-08-18"         # never date.today()
    assert time["periods"] == 230
    assert time["evidence"]["statistic"] == "columns[].min/max"


def test_a_monthly_column_is_grained_by_month():
    columns = [_column("period", "Utf8", lo={"Text": "2024-01"}, hi={"Text": "2026-08"})]
    facts = curation.facts_from_engine(_statistics(1000, columns), None, {"period": "varchar"},
                                       run_probe=lambda sql: ([32], {"mode": "context"}),
                                       table="cat.sch.t")
    time = curation.derive_time([{"column_name": "period", "data_type": "varchar"}], facts)
    assert time["grain"] == "month" and time["latest"] == "2026-08"


def test_daily_bounds_with_few_distinct_values_are_monthly():
    columns = [_column("period", "Utf8", lo={"Text": "2024-01-01"}, hi={"Text": "2026-08-01"})]
    facts = curation.facts_from_engine(_statistics(1000, columns), None, {"period": "varchar"},
                                       run_probe=lambda sql: ([32], {"mode": "context"}),
                                       table="cat.sch.t")
    time = curation.derive_time([{"column_name": "period", "data_type": "varchar"}], facts)
    assert time["grain"] == "month" and time["latest"] == "2026-08"


# ── the merge: idempotent, and a curator's edits survive ──────────────────────

def test_regenerate_is_idempotent():
    first = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    second = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    assert first == second
    assert _merge_spec(first, {}) == _merge_spec(second, {})


def test_curated_fields_win_and_derived_fields_refresh():
    derived = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    curated = {"dimensions": {"country": {"display_name": "Country", "aliases": ["nation"],
                                          "top_n": 10, "cardinality": 3}},
               "default_metric": "latency_ms"}
    merged = _merge_spec(derived, curated)
    entry = merged["dimensions"]["country"]
    assert entry["display_name"] == "Country"          # curated
    assert entry["aliases"] == ["nation"]              # curated
    assert entry["top_n"] == 10                        # curated
    assert entry["cardinality"] == 26                  # derived — the record's word, not the curator's
    assert entry["curated"] == ["aliases", "display_name", "top_n"]
    assert merged["default_metric"] == "latency_ms"
    assert merged["time"]["latest"] == "2026-08-18"    # derived section survives the merge


def test_curation_for_a_column_that_is_gone_is_dropped():
    derived = curation.derive(DATASET_COLUMNS, DATASET_METRICS, _facts(), engine=True)
    merged = _merge_spec(derived, {"dimensions": {"retired_column": {"display_name": "Gone"}},
                                   "default_metric": "Nothing"})
    assert "retired_column" not in merged["dimensions"]
    assert merged["default_metric"] in merged["metrics"]


def test_no_statistics_falls_back_to_the_name_heuristic():
    spec = _suggest_spec(DATASET_COLUMNS, DATASET_METRICS, shape=None, engine=False, facts=None)
    # every dataset dimension is kept, and nothing claims statistical evidence
    assert set(spec["dimensions"]) == {"country", "note", "usage_date"}
    assert "derived_from" not in spec
