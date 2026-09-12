"""The DLM in a conversation: ambiguity becomes a question, out-of-scope is
refused fast, follow-ups inherit the previous frame, and near-miss tokens
resolve by bounded edit distance. Everything deterministic; no model."""
import unittest
from contextlib import ExitStack
from unittest.mock import patch

from dlm import engine

DATASET = {
    "id": "7", "dataset_name": "Sales", "database_name": "OpenSource", "schema_name": "sales",
    "fact_table": "orders", "date_column": "order_date",
    "columns": [
        {"column_name": "region", "is_dimension": True},
        {"column_name": "customer", "is_dimension": True},
        {"column_name": "order_date", "is_dimension": False},
    ],
    "metrics": [
        {"name": "Gross revenue", "expression": "SUM(gross_revenue)"},
        {"name": "Net revenue", "expression": "SUM(net_revenue)"},
        {"name": "Orders", "expression": "COUNT(*)"},
    ],
}


class Harness(ExitStack):
    """Pins every collaborator ask() reaches for, so the tests exercise only its dialogue logic."""
    def __init__(self, routed=None, filters=None, native=None):
        super().__init__()
        self.routed = [{"dataset_id": "7", "score": 9.0}] if routed is None else routed
        self.filters = filters or []
        self.native = native

    def __enter__(self):
        super().__enter__()
        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: self.routed))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: DATASET if str(i) == "7" else None))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: {}))
        self.enter_context(patch.object(engine, "_resolve_entity_filters", lambda i, q, **kw: list(self.filters)))
        self.enter_context(patch.object(engine, "_serve_from_context", lambda *a, **k: None))
        self.enter_context(patch.object(engine, "_dataset_year_bounds", lambda i: (2023, 2026)))
        self.enter_context(patch.object(engine, "_metric_year_bounds", lambda *a, **k: (2023, 2026)))
        self.enter_context(patch.object(engine, "_context_hints", lambda *a, **k: []))
        self.enter_context(patch.object(engine, "_vocabulary_hit", lambda q: True))
        self.enter_context(patch.object(engine, "_native_catalog", lambda db: self.native))
        return self


class ClarificationTests(unittest.TestCase):
    def test_tied_metrics_ask_instead_of_picking_the_first(self):
        with Harness():
            result = engine.ask("revenue by region")
        self.assertFalse(result["ok"])
        self.assertEqual(result["reason"], "clarify")
        c = result["clarification"]
        self.assertEqual(c["kind"], "metric")
        self.assertEqual([o["id"] for o in c["options"]], ["Gross revenue", "Net revenue"])
        self.assertEqual(result["resume"]["question"], "revenue by region")

    def test_a_pinned_choice_completes_the_question(self):
        with Harness():
            result = engine.ask("revenue by region", choices={"metric": "Net revenue"})
        self.assertTrue(result["ok"])
        self.assertIn('SUM(net_revenue) AS "Net revenue"', result["sql"])
        self.assertIn('GROUP BY "region"', result["sql"])
        self.assertEqual(result["frame"]["metric"], "Net revenue")

    def test_a_distinctive_word_needs_no_question(self):
        with Harness():
            result = engine.ask("net revenue by region")
        self.assertTrue(result["ok"])
        self.assertIn("net_revenue", result["sql"])


class ScopeTests(unittest.TestCase):
    def test_nothing_in_any_vocabulary_is_refused_as_out_of_scope(self):
        with Harness(routed=[]), patch.object(engine, "_vocabulary_hit", lambda q: False), \
             patch.object(engine, "_dataset_names", lambda: ["Sales", "Taxi"]):
            result = engine.ask("write an email to my team about tomorrow's weather")
        self.assertEqual(result["reason"], "out_of_scope")
        self.assertEqual(result["datasets"], ["Sales", "Taxi"])

    def test_a_near_miss_stays_no_dataset_not_out_of_scope(self):
        with Harness(routed=[]):
            result = engine.ask("revenue")
        self.assertEqual(result["reason"], "no_dataset")


class FrameTests(unittest.TestCase):
    def test_follow_up_inherits_metric_and_grouping_and_changes_only_time(self):
        frame = {"dataset_id": "7", "metric": "Net revenue", "group_col": "region", "filters": [], "year": None, "relative_time": None}
        with Harness(routed=[]):
            result = engine.ask("what about 2024?", frame=frame)
        self.assertTrue(result["ok"], result)
        self.assertIn("net_revenue", result["sql"])
        self.assertIn('GROUP BY "region"', result["sql"])
        self.assertIn("2024", result["sql"])
        self.assertEqual(result["frame"]["year"], 2024)

    def test_follow_up_adds_a_filter_and_keeps_the_rest(self):
        frame = {"dataset_id": "7", "metric": "Net revenue", "group_col": "region", "filters": [], "year": 2024, "relative_time": None}
        with Harness(routed=[], filters=[{"column": "customer", "value": "Contoso"}]):
            result = engine.ask("filter that by customer Contoso", frame=frame)
        self.assertTrue(result["ok"], result)
        self.assertIn("\"customer\" = 'Contoso'", result["sql"])
        self.assertIn("2024", result["sql"])
        self.assertEqual(result["frame"]["filters"], [{"column": "customer", "value": "Contoso"}])

    def test_a_fresh_question_that_routes_elsewhere_ignores_the_frame(self):
        frame = {"dataset_id": "9", "metric": "Trips", "group_col": "borough", "filters": [], "year": None, "relative_time": None}
        with Harness():
            result = engine.ask("orders by region", frame=frame)
        self.assertEqual(result["dataset_id"], "7")
        self.assertIn("COUNT(*)", result["sql"])


class QueryPlaneTests(unittest.TestCase):
    def test_a_native_catalog_answer_is_marked_for_the_engine_and_unquoted(self):
        with Harness(native={"engine_catalog": "OpenSource"}):
            result = engine.ask("net revenue by region")
        self.assertTrue(result["engine"])
        self.assertIn('AS "Net revenue"', result["sql"])   # aliases with spaces stay quoted
        self.assertIn("FROM sales.orders", result["sql"])
        self.assertIn("GROUP BY region", result["sql"])

    def test_an_external_source_keeps_quoted_sql_for_the_pool(self):
        with Harness():
            result = engine.ask("net revenue by region")
        self.assertFalse(result["engine"])
        self.assertIn('GROUP BY "region"', result["sql"])


class GroupingTests(unittest.TestCase):
    DIMS = [{"column_name": "license"}, {"column_name": "segment"}, {"column_name": "platform"},
            {"column_name": "deployment"}, {"column_name": "acquisition_channel"}]

    def test_a_named_dimension_outranks_a_synonym_match(self):
        # "segment" reaches "license" only through a curated alias; naming the column wins outright.
        self.assertEqual(engine._group_by_candidates("rows scanned by license", self.DIMS, {"segment": ["license"]}),
                         ["license"])

    def test_two_named_dimensions_become_one_pair_group(self):
        self.assertEqual(engine._group_by_candidates("errors by platform and deployment", self.DIMS),
                         ["deployment|platform"])
        self.assertEqual(engine._group_by_candidates("errors by deployment, platform", self.DIMS),
                         ["deployment|platform"])

    def test_a_multi_word_column_is_matched_whole(self):
        self.assertEqual(engine._group_by_candidates("sessions by acquisition channel", self.DIMS),
                         ["acquisition_channel"])

    def test_a_pair_group_builds_two_column_sql(self):
        dataset = dict(DATASET, columns=DATASET["columns"] + [{"column_name": "channel", "is_dimension": True}])
        with Harness(), patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: dataset):
            result = engine.ask("orders by region and channel")
        self.assertTrue(result["ok"], result)
        self.assertIn('GROUP BY "channel", "region"', result["sql"])
        self.assertEqual(result["columns"], ["channel", "region", "Orders"])
        self.assertEqual(result["xAxis"], "channel")
        self.assertIn("by channel and region", result["title"])
        self.assertEqual(result["frame"]["group_col"], "channel|region")


class CorpusRegressionTests(unittest.TestCase):
    """Shapes the 80-question corpus caught on 2026-09-12."""

    def test_total_in_the_question_does_not_reach_a_metric_named_total(self):
        metrics = [{"name": "Total actions", "expression": "SUM(actions)"}, {"name": "Errors", "expression": "SUM(errors)"},
                   {"name": "Duration (sec)", "expression": "SUM(duration_sec)"}, {"name": "Sessions", "expression": "SUM(sessions)"}]
        ds = dict(DATASET, metrics=metrics)
        for q, want in [("errors in total", "Errors"), ("what is the total duration in seconds", "Duration (sec)"),
                        ("number of sessions by region", "Sessions")]:
            with Harness(), patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: ds):
                result = engine.ask(q)
            self.assertTrue(result["ok"], (q, result))
            self.assertEqual(result["frame"]["metric"], want, q)

    def test_a_time_phrase_does_not_pull_a_duration_metric(self):
        metrics = [{"name": "Errors", "expression": "SUM(errors)"}, {"name": "Duration (sec)", "expression": "SUM(duration_sec)"}]
        ds = dict(DATASET, metrics=metrics)
        with Harness(), patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: ds):
            result = engine.ask("errors last 7 days")
        self.assertEqual(result["frame"]["metric"], "Errors")
        self.assertRegex(result["sql"], r"\"order_date\" >= '\d{4}-\d{2}-\d{2}'")

    def test_a_superlative_with_a_dimension_is_a_top_one(self):
        with Harness():
            result = engine.ask("lowest net revenue region")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["group_col"], "region")
        self.assertEqual(result["frame"]["top_n"], 1)
        self.assertTrue(result["frame"]["sort_asc"])

    def test_a_value_in_several_columns_is_a_question_then_a_filter(self):
        hits = [{"element_key": "sales.orders.customer", "value_text": "Enterprise", "key_value": "Enterprise", "freq": 5},
                {"element_key": "sales.orders.region", "value_text": "Enterprise", "key_value": "Enterprise", "freq": 3}]
        index = lambda dataset_id, term, limit=5, exact_only=False: hits if term == "Enterprise" else []
        with patch.object(engine, "resolve_value", index), patch.object(engine, "_effective_spec", lambda i: {}):
            amb = []
            self.assertEqual(engine._resolve_entity_filters("7", "orders for Enterprise customers", ambiguous=amb), [])
            self.assertEqual(amb, [{"value": "Enterprise", "columns": ["customer", "region"]}])
            pinned = engine._resolve_entity_filters("7", "orders for Enterprise customers",
                                                    choices={"value": "region", "value_phrase": "enterprise"})
            self.assertEqual([(f["column"], f["value"]) for f in pinned], [("region", "Enterprise")])

    def test_sql_pasted_as_a_question_is_out_of_scope_with_a_hint(self):
        with Harness(), patch.object(engine, "_dataset_names", lambda: ["Sales"]):
            for q in ("select * from users", "DROP TABLE orders"):
                result = engine.ask(q)
                self.assertEqual(result["reason"], "out_of_scope", q)
                self.assertIn("SQL Lab", result["hint"])


class EditDistanceTests(unittest.TestCase):
    def test_a_typo_resolves_to_the_nearest_vocabulary_token(self):
        with Harness():
            result = engine.ask("net revnue by regoin")
        self.assertTrue(result["ok"], result)
        self.assertIn("net_revenue", result["sql"])
        self.assertIn('GROUP BY "region"', result["sql"])
        self.assertEqual(result["frame"]["group_col"], "region")


if __name__ == "__main__":
    unittest.main()
