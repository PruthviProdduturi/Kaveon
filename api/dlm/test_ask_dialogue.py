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
    def __init__(self, routed=None, filters=None, native=None, near=None):
        super().__init__()
        self.routed = [{"dataset_id": "7", "score": 9.0}] if routed is None else routed
        self.filters = filters or []
        self.native = native
        self.near = near or []

    def __enter__(self):
        super().__enter__()
        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: self.routed))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i: DATASET if str(i) == "7" else None))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: {}))
        self.enter_context(patch.object(engine, "_resolve_entity_filters", lambda i, q, **kw: list(self.filters)))
        self.enter_context(patch.object(engine, "_near_values", lambda i, t, limit=5: list(self.near)))
        self.enter_context(patch.object(engine, "_serve_from_context", lambda *a, **k: None))
        self.enter_context(patch.object(engine, "_context_answer", lambda *a, **k: None))   # no precomputed cells
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


# A native-catalog dataset shaped like "Product users" (2026-09-18): one fact
# table, nine declared dimensions, `locale` a column that is not one, and a
# COUNT(*) metric named Users. The value index, the precomputed answers and the
# suggested context spec are all in memory, so entity resolution, ranking and
# context serving run for real; only the metadata store is absent.
PRODUCT_USERS_DIMS = ["platform", "license", "segment", "industry", "region", "country",
                      "deployment", "acquisition_channel", "team_size"]
PRODUCT_USERS = {
    "id": "2", "dataset_name": "Product users", "database_name": "OpenSource",
    "schema_name": "kaveon_product", "table_name": "kaveon_events_users", "date_column": None,
    "columns": [{"table_name": "kaveon_events_users", "column_name": "user_id", "data_type": "bigint", "is_dimension": False},
                {"table_name": "kaveon_events_users", "column_name": "locale", "data_type": "varchar", "is_dimension": False}]
               + [{"table_name": "kaveon_events_users", "column_name": d, "data_type": "varchar", "is_dimension": True}
                  for d in PRODUCT_USERS_DIMS],
    "metrics": [{"name": "Users", "expression": "COUNT(*)", "metric_type": "count"},
                {"name": "Locales", "expression": "COUNT(DISTINCT locale)", "metric_type": "count_distinct"}],
}
PRODUCT_USERS_VALUES = {
    "platform": ["Desktop", "Mobile", "Web"],
    "license": ["Enterprise", "Free", "Professional", "Standard"],
    "segment": ["Enterprise", "Mid-Market", "SMB", "Startup"],
    "industry": ["Education", "Energy", "Financial Services", "Government", "Healthcare", "Logistics",
                 "Manufacturing", "Media", "Professional Services", "Real Estate", "Retail", "Technology"],
    "region": ["Africa", "Asia", "Europe", "North America", "Oceania", "South America"],
    "country": ["United States", "India", "Germany", "Brazil", "Japan", "United Kingdom", "France", "Canada"],
    "deployment": ["Cloud", "Hybrid", "On-Premise"],
    "acquisition_channel": ["Organic", "Referral", "Paid", "Partner", "Direct"],
    "team_size": ["Enterprise", "Large", "Medium", "Small", "Solo"],
}
PRODUCT_USERS_INDEX = [
    {"element_key": f"kaveon_product.kaveon_events_users.{col}", "value_text": v,
     "value_norm": engine._normalize(v), "key_column": col, "key_value": v, "freq": 1000 - i}
    for col, vals in PRODUCT_USERS_VALUES.items() for i, v in enumerate(vals)]
PRODUCT_USERS_ANSWERS = {
    ("Users", ""): {"columns": ["Users"], "rows": [[3000000]]},
    ("Users", "country"): {"columns": ["country", "Users"],
                           "rows": [["United States", 900000], ["India", 600000], ["Germany", 400000],
                                    ["Brazil", 350000], ["Japan", 300000], ["United Kingdom", 200000],
                                    ["France", 150000], ["Canada", 100000]]},
    ("Users", "platform"): {"columns": ["platform", "Users"],
                            "rows": [["Desktop", 1124707], ["Mobile", 940000], ["Web", 935293]]},
    ("Users", "acquisition_channel"): {"columns": ["acquisition_channel", "Users"],
                                       "rows": [["Organic", 999614], ["Referral", 667589], ["Paid", 666499],
                                                ["Partner", 333965], ["Direct", 332333]]},
    ("Users", "platform|region"): {"columns": ["platform", "region", "Users"],
                                   "rows": [["Desktop", "Europe", 290187], ["Mobile", "Europe", 242540],
                                            ["Web", "Europe", 241838], ["Desktop", "Asia", 300000]]},
}


class ProductUsersHarness(ExitStack):
    def __enter__(self):
        super().__enter__()
        spec = engine._suggest_spec(PRODUCT_USERS["columns"], PRODUCT_USERS["metrics"])

        def resolve(dataset_id, term, limit=5, exact_only=False, actor=None, role="Viewer"):
            norm = engine._normalize(term)
            norm = engine._VALUE_ALIASES.get(norm, norm)
            rows = [r for r in PRODUCT_USERS_INDEX if r["value_norm"] == norm]
            if not rows and not exact_only and len(norm) >= 4:
                rows = [r for r in PRODUCT_USERS_INDEX if r["value_norm"].startswith(norm)]
            return [engine._value_hit(r) for r in rows[:limit]]

        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: [{"dataset_id": "2", "score": 16.0}]))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id",
                                        lambda i: PRODUCT_USERS if str(i) == "2" else None))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: spec))
        self.enter_context(patch.object(engine, "resolve_value", resolve))
        self.enter_context(patch.object(engine, "_indexed_values", lambda i: list(PRODUCT_USERS_INDEX)))
        self.enter_context(patch.object(engine, "_context_answer",
                                        lambda i, metric, group: PRODUCT_USERS_ANSWERS.get((metric, group or ""))))
        self.enter_context(patch.object(engine, "_load_sketches", lambda i: {}))
        self.enter_context(patch.object(engine, "_context_hints", lambda *a, **k: []))
        self.enter_context(patch.object(engine, "_vocabulary_hit", lambda q: True))
        self.enter_context(patch.object(engine, "_native_catalog", lambda db: {"engine_catalog": "OpenSource"}))
        return self


class ProductUsersRankingTests(unittest.TestCase):
    def test_top_n_by_the_measure_groups_by_the_named_dimension(self):
        # "by users" names the measure; before, the lexicon's "acquisition ~ new user"
        # turned it into a grouping by acquisition_channel.
        with ProductUsersHarness():
            result = engine.ask("top 5 countries by users")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["group_col"], "country")
        self.assertEqual(result["frame"]["top_n"], 5)
        self.assertFalse(result["frame"]["sort_asc"])
        self.assertEqual([r[0] for r in result["rows"]], ["United States", "India", "Germany", "Brazil", "Japan"])
        self.assertEqual(result["title"], "Top 5 country by Users")
        self.assertIsNone(result["note"])

    def test_bottom_n_orders_ascending(self):
        with ProductUsersHarness():
            result = engine.ask("bottom 3 countries by users")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["group_col"], "country")
        self.assertEqual(result["frame"]["top_n"], 3)
        self.assertTrue(result["frame"]["sort_asc"])
        self.assertEqual([r[0] for r in result["rows"]], ["Canada", "France", "United Kingdom"])
        self.assertEqual(result["title"], "Bottom 3 country by Users")

    def test_lowest_n_and_highest_n_set_the_limit(self):
        with ProductUsersHarness():
            lowest = engine.ask("lowest 2 countries by users")
            highest = engine.ask("highest 4 countries by users")
        self.assertEqual((lowest["frame"]["top_n"], lowest["frame"]["sort_asc"]), (2, True))
        self.assertEqual((highest["frame"]["top_n"], highest["frame"]["sort_asc"]), (4, False))

    def test_a_ranking_over_the_second_metric_orders_by_that_metric(self):
        answers = dict(PRODUCT_USERS_ANSWERS)
        answers[("Locales", "country")] = {"columns": ["country", "Locales"],
                                           "rows": [["United States", 3], ["India", 9], ["Germany", 5]]}
        with ProductUsersHarness(), patch.object(engine, "_context_answer",
                                                 lambda i, metric, group: answers.get((metric, group or ""))):
            result = engine.ask("top 2 countries by locales")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["metric"], "Locales")
        self.assertEqual([r[0] for r in result["rows"]], ["India", "Germany"])

    def test_a_ranking_on_the_live_path_titles_and_limits(self):
        with ProductUsersHarness(), patch.object(engine, "_context_answer", lambda *a, **k: None):
            result = engine.ask("top 5 countries by users")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["route"], "live")
        self.assertIn("GROUP BY country ORDER BY Users DESC LIMIT 5", result["sql"])
        self.assertEqual(result["title"], "Top 5 country by Users")

    def test_by_the_measure_never_reaches_a_dimension_through_a_synonym(self):
        dims = [{"column_name": d} for d in PRODUCT_USERS_DIMS]
        spec = engine._suggest_spec(PRODUCT_USERS["columns"], PRODUCT_USERS["metrics"])
        self.assertEqual(engine._group_by_candidates("top 5 countries by users", dims,
                                                     engine._alias_index(spec, "dimensions"),
                                                     PRODUCT_USERS["metrics"], engine._alias_index(spec, "metrics")), [])
        self.assertEqual(engine._group_by_candidates("users by platform", dims, {}, PRODUCT_USERS["metrics"], {}),
                         ["platform"])
        self.assertEqual(engine._match_any_dim("top 5 countries by users", dims, {}, exclude={"users"}), "country")

    def test_the_earlier_questions_still_answer_as_before(self):
        with ProductUsersHarness():
            total = engine.ask("how many users are there")
            europe = engine.ask("users by platform in Europe")
            three = engine.ask("users by deployment and license and platform")
        self.assertEqual((total["title"], total["rows"], total["note"]), ("Users", [[3000000]], None))
        self.assertEqual(europe["title"], "Users by platform — Europe")
        self.assertEqual(europe["frame"]["filters"], [{"column": "region", "value": "Europe"}])
        self.assertEqual([r[0] for r in europe["rows"]], ["Desktop", "Mobile", "Web"])
        self.assertIsNone(europe["note"])
        self.assertEqual(three["reason"], "clarify")
        self.assertEqual(three["clarification"]["kind"], "dimension")
        self.assertEqual([o["id"] for o in three["clarification"]["options"]], ["platform", "license", "deployment"])


class ProductUsersUnresolvedTests(unittest.TestCase):
    def test_a_near_miss_value_asks_with_the_closest_values(self):
        with ProductUsersHarness():
            result = engine.ask("desktop users in finance")
        self.assertFalse(result["ok"])
        self.assertEqual(result["reason"], "clarify")
        c = result["clarification"]
        self.assertEqual(c["kind"], "value")
        self.assertIn('"finance" is not a value of platform, license', c["prompt"])
        self.assertIn("did you mean Financial Services?", c["prompt"])
        self.assertEqual(c["options"][0], {"id": "industry=Financial Services",
                                           "label": "industry = Financial Services", "description": ""})
        self.assertEqual(c["options"][-1]["id"], "skip")
        self.assertEqual(result["resume"], {"question": "desktop users in finance",
                                            "choices": {"value_phrase": "finance"}})

    def test_the_chosen_value_becomes_a_filter_and_the_desktop_filter_survives(self):
        with ProductUsersHarness():
            result = engine.ask("desktop users in finance",
                                choices={"value_phrase": "finance", "value": "industry=Financial Services"})
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["filters"], [{"column": "platform", "value": "Desktop"},
                                                      {"column": "industry", "value": "Financial Services"}])
        self.assertIsNone(result["note"])

    def test_skipping_the_word_answers_with_a_note(self):
        with ProductUsersHarness():
            result = engine.ask("desktop users in finance", choices={"value_phrase": "finance", "value": "skip"})
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["filters"], [{"column": "platform", "value": "Desktop"}])
        self.assertEqual(result["note"], '"finance" was left out of the answer.')

    def test_a_word_with_nothing_close_is_left_out_with_a_note(self):
        with ProductUsersHarness():
            result = engine.ask("desktop users in zzyzx")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["filters"], [{"column": "platform", "value": "Desktop"}])
        self.assertEqual(result["rows"], [[1124707]])
        self.assertEqual(result["note"], '"zzyzx" matched no value of platform, license, segment, industry, region, '
                                         'country, deployment, acquisition_channel, team_size and was left out of the answer.')

    def test_a_second_value_of_a_filtered_column_is_recognised_and_noted(self):
        with ProductUsersHarness():
            result = engine.ask("users in Germany and France")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["frame"]["filters"], [{"column": "country", "value": "Germany"}])
        self.assertEqual(result["note"], '"France" is a second country value; one value per dimension is applied (Germany).')

    def test_a_qualifier_before_the_measure_is_checked_too(self):
        with ProductUsersHarness():
            result = engine.ask("finance users by country")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["clarification"]["options"][0]["id"], "industry=Financial Services")

    def test_a_multi_word_typo_offers_the_value(self):
        with ProductUsersHarness():
            result = engine.ask("users in north amerca")
        self.assertEqual(result["reason"], "clarify")
        self.assertEqual(result["clarification"]["options"][0]["id"], "region=North America")

    def test_a_by_phrase_naming_a_column_that_is_not_a_dimension_is_explained(self):
        with ProductUsersHarness():
            result = engine.ask("users by locale")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["title"], "Users")
        self.assertIsNone(result["frame"]["group_col"])
        self.assertEqual(result["note"], "locale is a column of Product users but not one of its dimensions, "
                                         "so the answer is not broken down by it. Dimensions: platform, license, "
                                         "segment, industry, region, country, deployment, acquisition_channel, team_size.")

    def test_a_by_phrase_naming_nothing_is_noted(self):
        with ProductUsersHarness():
            result = engine.ask("users by colour")
        self.assertTrue(result["ok"], result)
        self.assertTrue(result["note"].startswith("No dimension matches the requested breakdown. Dimensions: platform"))

    def test_near_values_rank_stem_then_edit_then_prefix(self):
        with ProductUsersHarness():
            self.assertEqual([h["value"] for h in engine._near_values("2", "finance")], ["Financial Services"])
            self.assertEqual([h["value"] for h in engine._near_values("2", "japn")], ["Japan"])
            self.assertEqual([(h["column"], h["value"]) for h in engine._near_values("2", "enterprises")],
                             [("license", "Enterprise"), ("segment", "Enterprise"), ("team_size", "Enterprise")])
            self.assertEqual(engine._near_values("2", "zzyzx"), [])
            self.assertEqual(engine._near_values("2", "in"), [])


if __name__ == "__main__":
    unittest.main()
