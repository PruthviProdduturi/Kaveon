"""A measure is answered or declined, never swapped for another one.

This is the one failure the product is sold on not making, and an adversarial
pass over deliberately awkward datasets found two ways in. Both are fixed
here; the shapes stay as tests so neither comes back quietly.

**A shared synonym.** "churn by region" over a dataset holding only `revenue`
and `orders` answered revenue. Neither word appears in the other -- they met
at a synonym, because `churn` expands to `turnover` and so does `revenue`.
Both sides were expanded through the seed lexicon and then intersected, so any
two unrelated concepts sharing one synonym matched. A match now has to be
anchored: the shared word must be one the question said, or one the metric is
named or defined by.

**The breakdown scored as a measure.** "revenue by region" answered
`COUNT(DISTINCT region)` -- a metric called `Regions` -- because every word of
the question, the breakdown included, was scored against the measures. The
words after "by" say how to split the answer, never what to measure. They
cannot simply be dropped: in "top 5 countries by total deaths" the phrase
after "by" *is* the measure, which is why an earlier attempt at this broke
"revenue by region" the other way round. So the breakdown is identified rather
than assumed, from whichever side names a dimension of the dataset outright.

Fixing the ranking exposed a third way in, one layer down: with `churn`
scoring nothing, the default measure answered the question instead. A question
that names no measure *should* take the default -- "what is the total" means
the headline one -- but a question that named a measure this dataset does not
have is a different question, and the default answers it wrongly. So where
nothing scored, a word the dataset cannot account for refuses.

The unit tests below pin each rule. The ask-path tests at the end are the ones
that cannot go vacuous: they drive `ask` and read the statement that came
back, so the wiring between the rules and the answer stays honest.
"""

import unittest
from contextlib import ExitStack
from unittest.mock import patch

from dlm import engine


def _measure(name):
    return {"column_name": name, "is_dimension": False, "is_metric": True}


def _dimension(name):
    return {"column_name": name, "is_dimension": True, "is_metric": False}


REGIONS = [{"name": "Regions", "expression": "COUNT(DISTINCT region)",
            "metric_type": "count_distinct"}]


def _dims(columns):
    """What `ask` passes as the dataset's dimensions -- only the columns that
    are ones. Handing it every column makes a measure look like a breakdown."""
    return [c for c in columns if c.get("is_dimension")]


def _best(question, columns, named=()):
    """What the router resolves, the way `ask` resolves it: the breakdown's own
    words taken out before the measures are scored."""
    metrics = list(named) + engine._implicit_measures(columns, list(named))
    qset = set(engine._tokenize(question)) - engine._grouping_only_tokens(
        question, _dims(columns), metrics)
    ranked = engine._rank_metrics(qset, metrics)
    if not ranked:
        return None, 0
    best, score = ranked[0]
    return (best.get("name") or best.get("metric_name")), score


class AMatchMustBeAnchored(unittest.TestCase):
    """One side has to say the word. Expanding both and intersecting makes any
    shared synonym a match, however unrelated the two concepts are."""

    SALES = [_dimension("region"), _measure("revenue"), _measure("orders")]

    def test_a_measure_the_dataset_does_not_have_scores_nothing(self):
        name, score = _best("churn by region", self.SALES)
        self.assertEqual(score, 0, f"churn resolved to {name}")

    def test_the_shared_synonym_is_really_there(self):
        """Teeth: the test above is only meaningful because these two words do
        meet in the lexicon. If they stop sharing `turnover` it passes for the
        wrong reason."""
        self.assertTrue(engine._expand_tokens({"churn"}) & engine._expand_tokens({"revenue"}),
                        "churn and revenue no longer share a synonym")

    def test_an_honest_synonym_still_matches(self):
        """The question said "sales" and the metric expands to it, so one side
        says the word and the match stands."""
        name, score = _best("sales by region", self.SALES)
        self.assertEqual(name, "revenue")
        self.assertGreater(score, 0)

    def test_a_measure_named_outright_still_matches(self):
        self.assertEqual(_best("revenue by region", self.SALES)[0], "revenue")

    def test_a_curated_alias_anchors_on_the_metric_side(self):
        """An alias is a human statement about this metric, so it counts as the
        metric saying the word -- unlike a lexicon synonym."""
        metrics = [{"name": "revenue", "expression": "SUM(revenue)"}]
        ranked = engine._rank_metrics({"takings"}, metrics, {"revenue": ["takings"]})
        self.assertGreater(ranked[0][1], 0)


class TheBreakdownIsNotTheMeasure(unittest.TestCase):
    COLLIDE = [_dimension("region"), _measure("revenue")]
    COVID = [_dimension("country"), _measure("total_deaths"), _measure("new_deaths")]

    def test_a_dimension_named_after_the_cue_is_dropped(self):
        self.assertEqual(engine._grouping_only_tokens("revenue by region", _dims(self.COLLIDE)),
                         {"region"})

    def test_the_measure_wins_over_a_metric_named_for_the_dimension(self):
        self.assertEqual(_best("revenue by region", self.COLLIDE, REGIONS)[0], "revenue")

    def test_per_and_across_are_the_same_cue(self):
        for question in ("revenue per region", "revenue across region"):
            with self.subTest(question=question):
                self.assertEqual(_best(question, self.COLLIDE, REGIONS)[0], "revenue")

    def test_a_measure_named_after_the_cue_is_left_alone(self):
        """"top 5 countries by total deaths" is the form where the phrase after
        "by" is the measure. Dropping it would leave nothing to measure."""
        self.assertNotIn("deaths", engine._grouping_only_tokens(
            "top 5 countries by total deaths", _dims(self.COVID),
            engine._implicit_measures(self.COVID, [])))
        self.assertEqual(_best("top 5 countries by total deaths", self.COVID)[0], "total_deaths")

    def test_the_dimension_before_the_cue_is_dropped_instead(self):
        """"top 5 regions by revenue" names the measure after the cue, so the
        breakdown is the dimension in front of it -- and `Regions` must not win
        on it."""
        metrics = REGIONS + engine._implicit_measures(self.COLLIDE, REGIONS)
        self.assertEqual(
            engine._grouping_only_tokens("top 5 regions by revenue", _dims(self.COLLIDE), metrics),
            {"regions"})
        self.assertEqual(_best("top 5 regions by revenue", self.COLLIDE, REGIONS)[0], "revenue")

    def test_a_plural_breakdown_reaches_its_singular_column(self):
        self.assertEqual(engine._grouping_only_tokens("revenue by regions", _dims(self.COLLIDE)),
                         {"regions"})

    def test_a_word_used_on_both_sides_of_the_cue_survives(self):
        """"orders by order date" says "order" about the measure too, so the
        breakdown does not get to take it away."""
        columns = [_dimension("order_date"), _measure("orders")]
        self.assertNotIn("order", engine._grouping_only_tokens("orders by order date", _dims(columns)))
        self.assertEqual(_best("orders by order date", columns)[0], "orders")

    def test_a_question_with_no_cue_keeps_every_word(self):
        self.assertEqual(engine._grouping_only_tokens("how many regions", _dims(self.COLLIDE)), set())
        self.assertEqual(_best("how many regions", self.COLLIDE, REGIONS)[0], "Regions")

    def test_a_breakdown_this_dataset_has_no_dimension_for_is_left_alone(self):
        """Only a word that names a dimension outright is treated as the
        breakdown; a guess would strip words the measure needed."""
        self.assertEqual(engine._grouping_only_tokens("revenue by segment", _dims(self.COLLIDE)), set())


class AnUnknownMeasureWordRefuses(unittest.TestCase):
    """The layer under the ranking. Scoring nothing is correct for a question
    that names no measure, and the default is the right answer to it -- but not
    when the question named one this dataset does not have."""

    def test_a_word_the_dataset_cannot_account_for_is_reported(self):
        vocabulary = {"revenue", "orders", "region"}
        self.assertEqual(
            engine._unplaced_content_words("churn by region", vocabulary, set()),
            ["churn"])

    def test_a_synonym_of_a_measure_is_accounted_for(self):
        """"sales" over a dataset of `revenue` is a question this dataset can
        answer, so it is not a word it cannot place."""
        self.assertEqual(
            engine._unplaced_content_words("sales by region", {"revenue", "region"}, set()),
            [])

    def test_the_allowance_does_not_run_the_other_way(self):
        """`churn` and `revenue` share `turnover`. Expanding the question's
        word as well as the dataset's would let churn back in through it --
        which is the same defect, one layer down."""
        self.assertEqual(
            engine._unplaced_content_words("churn by region", {"revenue", "region"}, set()),
            ["churn"])

    def test_a_column_name_a_question_inflects_is_accounted_for(self):
        self.assertEqual(
            engine._unplaced_content_words("orders by regions", {"orders", "region"}, set()),
            [])

    def test_a_resolved_value_is_accounted_for(self):
        """A word the value index already placed as a filter is not unplaced."""
        self.assertEqual(
            engine._unplaced_content_words("revenue in germany", {"revenue"}, {"germany"}),
            [])

    def test_short_words_are_never_reported(self):
        self.assertEqual(engine._unplaced_content_words("sum of x by region", {"region"}, set()),
                         [])


SALES_DATASET = {
    "id": "2", "dataset_name": "Sales", "database_name": "OpenSource",
    "schema_name": "kaveon_product", "table_name": "sales", "date_column": None,
    "columns": [_dimension("region"), _measure("revenue"), _measure("orders")],
    "metrics": REGIONS,
}
SALES_INDEX = [
    {"element_key": "kaveon_product.sales.region", "value_text": value,
     "value_norm": engine._normalize(value), "key_column": "region",
     "key_value": value, "freq": 100 - i}
    for i, value in enumerate(["Africa", "Asia", "Europe"])
]


class SalesHarness(ExitStack):
    """`ask` end to end over a dataset built for the collision: a measure
    (`revenue`) whose breakdown dimension (`region`) is also the subject of a
    named metric (`Regions`). The Engine is stood in for, so each test can read
    the statement the question actually produced."""

    def __enter__(self):
        super().__enter__()
        spec = engine._suggest_spec(SALES_DATASET["columns"], SALES_DATASET["metrics"])
        self.statements = []

        def answer(*args, **kwargs):
            metric_name, group_cols = args[5], args[7]
            statement = args[23]
            sql = statement(engine.dialects.ENGINE)
            self.statements.append(sql)
            return {"ok": True, "dataset_id": "2", "dataset_name": "Sales",
                    "database": "OpenSource", "schema_name": "kaveon_product", "sql": sql,
                    "engine": True, "executed": True, "route": "live",
                    "chartType": "bar" if group_cols else "kpi",
                    "xAxis": group_cols[0] if group_cols else None, "yAxis": metric_name,
                    "title": metric_name, "columns": list(group_cols) + [metric_name],
                    "rows": [["Europe", 10]] if group_cols else [[10]],
                    "filters": [], "year": None, "note": None, "approx": False,
                    "confidence": 1.0}

        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: [{"dataset_id": "2", "score": 16.0}]))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id",
                                        lambda i, *a, **k: SALES_DATASET if str(i) == "2" else None))
        self.enter_context(patch.object(engine.datasets_svc, "source_binding", lambda s: {"table_id": "t"}))
        self.enter_context(patch.object(engine, "_engine_binding", lambda ds, native=None: {
            "table_id": "t", "catalog": "OpenSource", "schema": "kaveon_product", "table": "sales"}))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: spec))
        self.enter_context(patch.object(engine, "_resolve_entity_filters", lambda i, q, **kw: []))
        self.enter_context(patch.object(engine, "_near_values", lambda i, t, limit=5: []))
        self.enter_context(patch.object(engine, "_indexed_values", lambda i: list(SALES_INDEX)))
        self.enter_context(patch.object(engine, "_dataset_year_bounds", lambda i: (2026, 2026)))
        self.enter_context(patch.object(engine, "_vocabulary_hit", lambda q: True))
        self.enter_context(patch.object(engine, "_native_catalog", lambda db: None))
        self.enter_context(patch.object(engine, "_answer_on_engine", answer))
        return self


class TheAskPathAnswersTheMeasureAsked(unittest.TestCase):
    """The end a reader sees. Each of these fails if the separation or the
    anchoring is taken out of the ask path, which the unit tests above cannot
    detect on their own."""

    def _ask(self, question):
        with SalesHarness() as harness:
            result = engine.ask(question)
        return result, (harness.statements[0] if harness.statements else "")

    def test_the_breakdown_does_not_become_the_measure(self):
        result, statement = self._ask("revenue by region")
        self.assertTrue(result["ok"], result)
        self.assertIn("SUM(revenue)", statement)
        self.assertNotIn("COUNT(DISTINCT region)", statement)

    def test_a_ranking_measures_what_comes_after_the_cue(self):
        result, statement = self._ask("top 3 regions by revenue")
        self.assertTrue(result["ok"], result)
        self.assertIn("SUM(revenue)", statement)
        self.assertEqual(result["frame"]["top_n"], 3)

    def test_an_honest_synonym_reaches_the_measure(self):
        _result, statement = self._ask("sales by region")
        self.assertIn("SUM(revenue)", statement)

    def test_a_measure_this_dataset_does_not_have_is_refused_by_name(self):
        result, statement = self._ask("churn by region")
        self.assertFalse(result["ok"], result)
        self.assertEqual(statement, "", "a refused question must run nothing")
        self.assertIn("churn", (result.get("why") or "").lower())

    def test_a_question_naming_no_measure_still_takes_the_default(self):
        """The refusal must not swallow the case the default exists for."""
        result, statement = self._ask("what is the total")
        self.assertTrue(result["ok"], result)
        self.assertTrue(statement)

    def test_counting_the_dimension_is_still_reachable_when_asked_for(self):
        _result, statement = self._ask("how many regions")
        self.assertIn("COUNT(DISTINCT region)", statement)


if __name__ == "__main__":
    unittest.main()
