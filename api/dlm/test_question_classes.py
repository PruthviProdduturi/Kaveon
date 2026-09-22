"""The question classes — each one named, each one answered from statements.

The harness stands in for the Engine: it takes the slot set `ask` resolved
and answers it from a small in-memory table, recording every statement it was
asked for. So each test says "this sentence is this class, it ran these
statements, and it reads like this" without a stack.
"""
import unittest
from contextlib import ExitStack
from unittest.mock import patch

from dlm import classes, engine

DATASET = {
    "id": "7", "dataset_name": "Product usage", "database_name": "OpenSource",
    "schema_name": "kaveon_product", "fact_table": "usage", "table_name": "usage",
    "date_column": "usage_date",
    "columns": [
        {"table_name": "usage", "column_name": "region", "is_dimension": True},
        {"table_name": "usage", "column_name": "country", "is_dimension": True},
        {"table_name": "usage", "column_name": "usage_date", "is_dimension": False},
        {"table_name": "usage", "column_name": "queries_run", "is_dimension": False},
        {"table_name": "usage", "column_name": "errors", "is_dimension": False},
        {"table_name": "usage", "column_name": "user_id", "is_dimension": False},
    ],
    "metrics": [
        {"name": "Queries run", "expression": "SUM(queries_run)"},
        {"name": "Errors", "expression": "SUM(errors)"},
        {"name": "Distinct users", "expression": "COUNT(DISTINCT user_id)"},
    ],
}

SPEC = {
    "metrics": {"Queries run": {"additive": True, "default": True},
                "Errors": {"additive": True},
                "Distinct users": {"additive": False, "approximate": True}},
    "dimensions": {"region": {"precompute": True}, "country": {"precompute": True}},
    "default_metric": "Queries run",
    "time": {"column": "usage_date", "min": "2025-01-01", "max": "2026-08-18",
             "grain": "month", "latest": "2026-08-18", "periods": 230},
    "derived_from": {"source": "engine_statistics"},
    "value_aliases": {},
}

# The table the harness answers from: (metric, grouping, window) -> rows.
TABLE = {
    ("Queries run", (), None): [[900]],
    ("Queries run", (), ("2026-08-01", "2026-09-01")): [[120]],
    ("Queries run", (), ("2026-07-01", "2026-08-01")): [[100]],
    ("Queries run", (), ("2026-01-01", "2027-01-01")): [[900]],
    ("Queries run", (), ("2025-01-01", "2026-01-01")): [[600]],
    ("Queries run", ("region",), None): [["Europe", 500], ["Asia", 300], ["Africa", 100]],
    ("Queries run", ("region", "country"), None): [
        ["Europe", "France", 300], ["Europe", "Spain", 200],
        ["Asia", "India", 250], ["Asia", "Japan", 50],
        ["Africa", "Kenya", 100]],
    ("Errors", (), None): [[45]],
    ("Errors", ("region",), None): [["Europe", 20], ["Asia", 15], ["Africa", 10]],
}


class Harness(ExitStack):
    def __init__(self, filters=None, spec=None):
        super().__init__()
        self.filters = filters or []
        self.spec = SPEC if spec is None else spec
        self.statements = []

    def _answer(self, *args, **kwargs):
        (_dataset_id, ds, _binding, _spec, _metric, metric_name, metric_expr, group_cols,
         _time_group, filters, _year, _month, _relative, _date_column, _columns, top_n,
         limit_n, sort_asc, _routed, _principal, _role, _question, _shifted, statement) = args[:24]
        window = kwargs.get("window")
        key = (metric_name, tuple(group_cols), tuple(window) if window else None)
        rows = [list(r) for r in TABLE.get(key, [])]
        if filters:
            rows = rows[:1]
        if top_n:
            rows = sorted(rows, key=lambda r: -(r[-1] or 0))[:top_n]
        sql = statement(engine.dialects.ENGINE)
        self.statements.append(sql)
        return {
            "ok": True, "dataset_id": "7", "dataset_name": ds.get("dataset_name"),
            "database": "OpenSource", "schema_name": "kaveon_product", "sql": sql,
            "engine": True, "executed": True, "from_context": True, "route": "context",
            "chartType": "bar" if group_cols else "kpi",
            "xAxis": group_cols[0] if group_cols else None, "yAxis": metric_name,
            "title": metric_name, "columns": list(group_cols) + [metric_name], "rows": rows,
            "filters": filters, "year": None, "note": None, "approx": False, "confidence": 1.0,
            "evidence": {"sql": sql, "lane": "context", "rows": len(rows),
                         "source": {"kind": "engine", "table_id": "t"},
                         "source_version": {"kind": "listing", "identity_sha256": "abc"},
                         "reproduce": {"sql": sql, "engine": True}},
        }

    def __enter__(self):
        super().__enter__()
        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: [{"dataset_id": "7", "score": 9.0}]))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: DATASET))
        self.enter_context(patch.object(engine.datasets_svc, "source_binding",
                                        lambda s: {"table_id": "t"}))
        self.enter_context(patch.object(engine, "_engine_binding", lambda ds, native=None: {
            "table_id": "t", "catalog": "OpenSource", "schema": "kaveon_product", "table": "usage"}))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: self.spec))
        self.enter_context(patch.object(engine, "_resolve_entity_filters",
                                        lambda i, q, **kw: list(self.filters)))
        self.enter_context(patch.object(engine, "_near_values", lambda i, t, limit=5: []))
        self.enter_context(patch.object(engine, "_dataset_year_bounds", lambda i: (2026, 2026)))
        self.enter_context(patch.object(engine, "_vocabulary_hit", lambda q: True))
        self.enter_context(patch.object(engine, "_native_catalog", lambda db: None))
        self.enter_context(patch.object(engine, "_answer_on_engine", self._answer))
        return self


class BaseClassTests(unittest.TestCase):
    def test_a_grand_total_is_the_total_class_with_its_sentence(self):
        with Harness():
            result = engine.ask("total queries run")
        self.assertEqual(result["question_class"], "total")
        self.assertEqual(result["answer"], "Queries run is 900.")

    def test_a_breakdown_is_named_and_leads_with_its_top_rows(self):
        with Harness():
            result = engine.ask("queries run by region")
        self.assertEqual(result["question_class"], "breakdown")
        self.assertIn("Europe 500", result["answer"])

    def test_a_ranking_is_the_top_n_class(self):
        with Harness():
            result = engine.ask("top 2 region by queries run")
        self.assertEqual(result["question_class"], "top_n")
        self.assertTrue(result["answer"].startswith("Top 2 region by Queries run"))

    def test_a_distinct_count_is_labelled_approximate(self):
        with Harness():
            result = engine.ask("distinct users")
        self.assertEqual(result["question_class"], "distinct_total")
        self.assertIn("approximate", result["answer"])


class ComparisonTests(unittest.TestCase):
    def test_vs_last_month_compares_two_windows_of_one_statement(self):
        with Harness() as harness:
            result = engine.ask("queries run vs last month")
        self.assertEqual(result["question_class"], "comparison_period")
        self.assertEqual(result["comparison"]["current"], 120)
        self.assertEqual(result["comparison"]["previous"], 100)
        self.assertEqual(result["comparison"]["delta"], 20)
        self.assertAlmostEqual(result["comparison"]["percent_change"], 20.0)
        self.assertIn("up 20", result["answer"])
        self.assertEqual(len(harness.statements), 2)
        self.assertIn("2026-08-01", harness.statements[0])
        self.assertIn("2026-07-01", harness.statements[1])

    def test_the_period_compared_is_the_latest_in_the_data_not_today(self):
        with Harness() as harness:
            engine.ask("queries run vs last month")
        # the dataset stops on 2026-08-18, so August is "current"
        self.assertIn("2026-08-01", harness.statements[0])
        self.assertIn("2026-09-01", harness.statements[0])

    def test_year_over_year_compares_the_same_span_a_year_earlier(self):
        with Harness() as harness:
            result = engine.ask("queries run year over year")
        self.assertEqual(result["question_class"], "year_over_year")
        self.assertEqual(result["comparison"]["period"], "2026")
        self.assertEqual(result["comparison"]["previous_period"], "2025")
        self.assertEqual(result["comparison"]["current"], 900)
        self.assertEqual(result["comparison"]["previous"], 600)
        self.assertIn("year over year", result["answer"])

    def test_a_comparison_keeps_both_statements_in_its_evidence(self):
        with Harness():
            result = engine.ask("queries run vs last month")
        evidence = result["evidence"]
        self.assertEqual(evidence["question_class"], "comparison_period")
        self.assertEqual(len(evidence["composed_of"]), 2)
        self.assertEqual(len(evidence["reproduce"]["statements"]), 2)
        self.assertEqual(evidence["lane"], "context")

    def test_a_previous_period_the_data_does_not_hold_is_said_not_invented(self):
        spec = dict(SPEC, time={**SPEC["time"], "min": "2026-08-01"})
        with Harness(spec=spec) as harness:
            result = engine.ask("queries run vs last month")
        self.assertEqual(len(harness.statements), 1)
        self.assertIn("no July 2026 to compare it with", result["answer"])


class ShareRatioTests(unittest.TestCase):
    def test_share_of_total_is_the_slice_over_the_whole(self):
        with Harness(filters=[{"column": "region", "value": "Europe"}]):
            result = engine.ask("what share of queries run is Europe")
        self.assertEqual(result["question_class"], "share_of_total")
        self.assertEqual(result["share"]["total"], 900)
        self.assertIn("%", result["answer"])

    def test_a_share_breakdown_gives_every_slice_a_percentage(self):
        with Harness():
            result = engine.ask("percentage of queries run by region")
        self.assertEqual(result["question_class"], "share_of_total")
        self.assertEqual(result["columns"], ["region", "Queries run", "share %"])
        self.assertAlmostEqual(result["rows"][0][2], 500 / 900 * 100)

    def test_a_ratio_divides_two_measures_over_the_same_slots(self):
        with Harness() as harness:
            result = engine.ask("errors per queries run")
        self.assertEqual(result["question_class"], "ratio")
        self.assertEqual(result["ratio"]["numerator"], 45)
        self.assertEqual(result["ratio"]["denominator"], 900)
        self.assertAlmostEqual(result["ratio"]["value"], 0.05)
        self.assertEqual(len(harness.statements), 2)


class WithinAndExistenceTests(unittest.TestCase):
    def test_top_n_within_ranks_inside_each_outer_group(self):
        with Harness():
            result = engine.ask("top 1 country by queries run in each region")
        self.assertEqual(result["question_class"], "top_n_within")
        self.assertEqual(result["columns"], ["region", "country", "Queries run", "rank"])
        winners = {row[0]: row[1] for row in result["rows"]}
        self.assertEqual(winners, {"Europe": "France", "Asia": "India", "Africa": "Kenya"})

    def test_existence_counts_the_groups_that_clear_the_threshold(self):
        with Harness():
            result = engine.ask("how many region have more than 150 queries run")
        self.assertEqual(result["question_class"], "existence")
        self.assertEqual(result["existence"]["matched"], 2)
        self.assertEqual(result["existence"]["considered"], 3)
        self.assertTrue(result["answer"].startswith("2 of 3 region have Queries run above 150"))


class VagueTests(unittest.TestCase):
    def test_current_usage_resolves_to_the_headline_measure_at_the_latest_period(self):
        with Harness() as harness:
            result = engine.ask("what is current usage")
        self.assertEqual(result["question_class"], "vague_default")
        self.assertEqual(result["yAxis"], "Queries run")
        self.assertIn("2026-08-01", harness.statements[0])
        self.assertIn("August 2026", result["answer"])
        # the answer says which defaults it used rather than assuming them silently
        self.assertIn("headline measure", result["note"])


class RegistryTests(unittest.TestCase):
    def test_every_class_the_router_can_produce_is_documented(self):
        documented = {c["name"] for c in classes.documented()}
        self.assertIn("comparison_period", documented)
        for entry in classes.documented():
            self.assertTrue(entry["description"] and entry["sql_shape"] and entry["sentence"])

    def test_period_arithmetic_is_calendar_correct(self):
        self.assertEqual(classes.period_bounds("2026-02", "month"), ("2026-02-01", "2026-03-01"))
        self.assertEqual(classes.period_bounds("2024-02", "month"), ("2024-02-01", "2024-03-01"))
        self.assertEqual(classes.previous_period("2026-01", "month"), "2025-12")
        self.assertEqual(classes.previous_period("2026-08", "month", years_back=1), "2025-08")
        self.assertEqual(classes.period_label("2026-08", "month"), "August 2026")

    def test_base_class_naming_is_a_total_order(self):
        self.assertEqual(classes.base_class(group_cols=[], filters=[], time_group=None,
                                            period=None, top_n=None, distinct=False), "total")
        self.assertEqual(classes.base_class(group_cols=["a"], filters=[{"column": "b"}],
                                            time_group=None, period=None, top_n=None,
                                            distinct=False), "filter_breakdown")
        self.assertEqual(classes.base_class(group_cols=["a"], filters=[], time_group=None,
                                            period=None, top_n=5, distinct=False), "top_n")
        self.assertEqual(classes.base_class(group_cols=[], filters=[], time_group="d",
                                            period=None, top_n=None, distinct=True), "trend")


if __name__ == "__main__":
    unittest.main()
