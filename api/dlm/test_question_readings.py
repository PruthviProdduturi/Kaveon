"""More words that carried two meanings, and the readings they stole.

`test_question_scope_and_redraw.py` pinned the first two of these — "across"
as scope rather than grouping, "chart" as a request to draw rather than a
measure. These are the rest of the same family, each one a token the resolver
read one way when the question meant the other:

* **"by" in "divided by".** The "by" belongs to the operator. Read as a
  grouping cue, "energy consumption divided by population" came back as "No
  dimension of Climate energy matches the requested breakdown — break it down
  by which one?", a question whose every answer is wrong.

* **"per" in "per <dimension>".** A ratio's denominator was resolved against
  the measures only. With a metric called "Countries tracked"
  (`COUNT(DISTINCT country)`), "energy consumption per country" — a plain
  breakdown — became "Energy consumption per Countries tracked".

* **"per" in "per 100k".** A cue followed by a number names no dimension, and
  reading it as one produced the same unanswerable breakdown question.

* **"per cent" spelled apart.** `_SHARE` knew "percent" and not "per cent", so
  "what per cent of energy consumption is in India" lost the share reading,
  `_RATIO` claimed it with "cent" as the denominator, resolved neither measure,
  and the plain total was returned as the answer to a percentage.

* **"what percentage of X have more than N".** Both the share class and the
  existence class read it; share was tested first and answered "Energy
  consumption is 100.0% of Energy consumption" — the measure over itself,
  because the question named no slice.

* **"by itself" / "in aggregate".** Scope idioms in the first person. The word
  after the cue is a pronoun, not a dimension, so these asked which dimension
  to break down by; "in aggregate" went further and was refused outright,
  because "aggregate" looked like a dimension value the dataset could not
  place.

* **"total" as a path to a dimension.** "total" is a seed synonym of "count",
  "count" shares four letters with "country", and `_match_any_dim`'s prefix
  tier accepts both — so "total deaths by" was answered as a breakdown by
  country, a dimension the question never mentions.

* **"average" as a routing signal.** A quantifier discounted by the metric
  matcher was worth x3 to the router, so a dataset whose measure is named
  "Average latency" scored on the word "average" alone.

Each case below carries its negatives, because the negatives are what stop a
fix taking a legitimate question with it.
"""

import json
import sys
import unittest
from contextlib import ExitStack
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import classes, engine

# A dataset shaped like the one the defects were found on: a dimension whose
# name is also a word of a measure's name, and two additive measures so a
# ratio is possible at all.
DATASET = {
    "id": "7", "dataset_name": "Climate energy", "database_name": "OpenSource",
    "schema_name": "climate", "fact_table": "energy", "date_column": "year_date",
    "columns": [
        {"column_name": "country", "is_dimension": True},
        {"column_name": "region", "is_dimension": True},
        {"column_name": "year_date", "is_dimension": False, "data_type": "date"},
    ],
    "metrics": [
        {"name": "Energy consumption", "expression": "SUM(primary_energy)"},
        {"name": "Population", "expression": "SUM(population)"},
        {"name": "Countries tracked", "expression": "COUNT(DISTINCT country)"},
    ],
}

DIMS = [{"column_name": "country"}, {"column_name": "region"}]
MEASURE_NAMES = [m["name"] for m in DATASET["metrics"]]


class Harness(ExitStack):
    """Everything `ask` reaches for, pinned, so these exercise the reading of
    the question and nothing else. Nothing is executed: the live lane returns
    the statement it would run, which is what the assertions read."""

    def __init__(self, dataset=None, filters=None):
        super().__init__()
        self.dataset = dataset or DATASET
        self.filters = filters or []

    def __enter__(self):
        super().__enter__()
        for name, value in (
            ("ensure_tables", lambda: None),
            ("route", lambda q, limit=1: [{"dataset_id": "7", "score": 9.0}]),
            ("_effective_spec", lambda i: {}),
            ("_resolve_entity_filters", lambda i, q, **kw: list(self.filters)),
            ("_near_values", lambda i, t, limit=5: []),
            ("_serve_from_context", lambda *a, **k: None),
            ("_context_answer", lambda *a, **k: None),
            ("_context_hints", lambda *a, **k: []),
            ("_vocabulary_hit", lambda q: True),
            ("_native_catalog", lambda db: None),
            ("_load_sketches", lambda i: {}),
            ("_index_examples", lambda i, limit=4: ["India"]),
            ("_dataset_year_bounds", lambda i: (2000, 2023)),
            ("_metric_year_bounds", lambda *a, **k: (2000, 2023)),
            ("_warehouse_evidence", lambda *a, **k: None),
        ):
            self.enter_context(patch.object(engine, name, value))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id",
                                        lambda i: self.dataset))
        return self


def ask(question, **kw):
    with Harness(**kw):
        return engine.ask(question)


def derived(question, *, has_time=True, has_group=False):
    return classes.detect_derived(question, has_time=has_time, dimensions=["country", "region"],
                                 measures=MEASURE_NAMES, has_group=has_group,
                                 explicit_metric=True, explicit_period=False)


class TheByOfDividedByIsNotAGroupingCue(unittest.TestCase):
    def test_arithmetic_by_is_not_a_cue(self):
        for question in ("energy consumption divided by population",
                         "revenue divided by orders",
                         "population multiplied by two"):
            self.assertIsNone(engine._GROUPING_CUE_RE.search(question), question)
            self.assertIsNone(engine._by_phrase(question), question)

    def test_every_other_by_still_is_one(self):
        """The negatives that matter: a breakdown is still asked for with the
        same word, and some of those phrasings end in "by" too."""
        for question, phrase in (("energy by country", "country"),
                                 ("energy split by region", "region"),
                                 ("energy broken down by region", "region"),
                                 ("energy grouped by country", "country"),
                                 ("energy per region", "region"),
                                 ("energy across countries", "countries"),
                                 ("energy for each region", "region")):
            self.assertIsNotNone(engine._GROUPING_CUE_RE.search(question), question)
            self.assertEqual(engine._by_phrase(question), phrase, question)

    def test_a_division_is_a_ratio(self):
        intent = derived("energy consumption divided by population")
        self.assertEqual(intent.name, "ratio")
        self.assertEqual(intent.params["numerator_phrase"], "energy consumption")
        self.assertEqual(intent.params["denominator_phrase"], "population")

    def test_a_division_is_answered_rather_than_questioned_back(self):
        result = ask("energy consumption divided by population")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["question_class"], "ratio")


class PerADimensionIsAGrouping(unittest.TestCase):
    def test_a_phrase_that_names_a_dimension_outright(self):
        for phrase in ("country", "countries", "region", "by region"):
            self.assertTrue(engine._names_dimension(phrase, DIMS), phrase)

    def test_a_phrase_that_only_resembles_one(self):
        """`_names_dimension` must not reach through the seed lexicon or the
        prefix guess, or it would cancel a legitimate ratio: "errors per
        counter" is not "per country"."""
        for phrase in ("capita", "queries run", "counter", "nation state",
                       "population", "person"):
            self.assertFalse(engine._names_dimension(phrase, DIMS), phrase)

    def test_per_country_is_a_breakdown_not_a_ratio(self):
        result = ask("energy consumption per country")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["question_class"], "breakdown")
        self.assertEqual(result["frame"]["group_col"], "country")
        self.assertNotIn("Countries tracked", result["sql"])

    def test_per_a_measure_is_still_a_ratio(self):
        result = ask("energy consumption per population")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["question_class"], "ratio")


class AGroupingCueNeedsAWordAfterIt(unittest.TestCase):
    def test_a_number_after_the_cue_is_not_a_dimension(self):
        for question in ("deaths per 100k", "deaths per 1 million people",
                         "energy per 1000 residents"):
            self.assertIsNone(engine._GROUPING_CUE_RE.search(question), question)

    def test_a_normalisation_is_not_turned_into_a_breakdown_question(self):
        single = dict(DATASET, metrics=[DATASET["metrics"][0]])
        result = ask("energy consumption per 100k", dataset=single)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["question_class"], "total")

    def test_a_word_after_the_cue_still_asks_for_one(self):
        for question in ("deaths by country", "deaths per region",
                         "deaths across countries", "deaths for each region"):
            self.assertIsNotNone(engine._GROUPING_CUE_RE.search(question), question)


class PerCentIsPercent(unittest.TestCase):
    def test_the_share_lead_in_spelled_apart(self):
        for question in ("what per cent of energy consumption is in India",
                         "per cent of energy consumption by region",
                         "energy consumption as a per cent of the total",
                         "what percent of energy consumption is in India"):
            self.assertIsNotNone(classes._SHARE.search(question), question)

    def test_per_is_not_a_share_on_its_own(self):
        """"per" is the ratio cue and "cent" is the only word that turns it
        into the share one, so the ordinary per-phrases must not match."""
        for question in ("energy consumption per capita",
                         "energy consumption per country",
                         "errors per query",
                         "energy consumption per region per year"):
            self.assertIsNone(classes._SHARE.search(question), question)


class WhatPercentageOfThemHaveIsACount(unittest.TestCase):
    def test_the_percentage_form_is_an_existence_count(self):
        for question in ("what percentage of countries have more than 1000 energy consumption",
                         "what percent of countries have over 1000 energy consumption",
                         "what per cent of countries have at least 1000 energy consumption",
                         "what share of countries have more than 1000 energy consumption",
                         "what proportion of countries have more than 1000 energy consumption"):
            intent = derived(question)
            self.assertEqual(intent.name, "existence", question)
            self.assertTrue(intent.params["as_share"], question)

    def test_how_many_is_the_same_class_without_the_proportion(self):
        intent = derived("how many countries have more than 1000 energy consumption")
        self.assertEqual(intent.name, "existence")
        self.assertFalse(intent.params["as_share"])

    def test_a_share_with_no_threshold_is_still_a_share(self):
        """The negatives: a question that asks for a proportion of a MEASURE
        rather than a count of groups must keep the share reading."""
        for question in ("what share of energy consumption is in India",
                         "percentage of energy consumption by region",
                         "energy consumption as a share of the total",
                         "what per cent of energy consumption is in India"):
            self.assertEqual(derived(question).name, "share_of_total", question)

    def test_a_share_needs_a_slice_to_be_a_share_of(self):
        """With no filter and no breakdown the part and the whole are the same
        statement, and the composed answer read "X is 100.0% of X"."""
        result = ask("what share of energy consumption")
        self.assertTrue(result["ok"], result)
        self.assertNotEqual(result["question_class"], "share_of_total")


class ScopeIdiomsInTheFirstPerson(unittest.TestCase):
    def test_the_measure_alone_is_a_scope(self):
        for question in ("energy by itself",
                         "deaths by themselves",
                         "energy on its own",
                         "deaths on their own",
                         "energy in aggregate",
                         "energy as a whole",
                         "energy across the board",
                         "energy in its entirety"):
            self.assertTrue(engine._asks_for_the_whole(question), question)

    def test_a_dimension_after_the_cue_is_still_a_grouping(self):
        for question in ("energy by item",
                         "energy by itemised region",
                         "energy by country",
                         "energy across countries",
                         "energy per region",
                         "energy on the web platform"):
            self.assertFalse(engine._asks_for_the_whole(question), question)

    def test_a_scope_phrase_leaves_the_question(self):
        """Cancelling the grouping cue was half the fix. The phrase's own words
        were still read as data: "world" is the object of a filter preposition,
        so the question was refused for a value the dataset could not place."""
        for question, gone in (("total energy in the world", "world"),
                               ("energy in aggregate", "aggregate"),
                               ("energy in its entirety", "entirety"),
                               ("energy worldwide", "worldwide")):
            self.assertNotIn(gone, engine._strip_scope_phrases(question).lower(), question)

    def test_a_real_filter_value_is_left_where_it_is(self):
        for question in ("energy in Europe", "energy in the Nordics",
                         "energy for India", "energy in world class datacentres"):
            self.assertEqual(engine._strip_scope_phrases(question), question, question)

    def test_the_whole_is_answered_rather_than_refused(self):
        for question in ("total energy consumption in the world",
                         "energy consumption in aggregate",
                         "energy consumption by itself",
                         "total energy consumption across the world"):
            result = ask(question)
            self.assertTrue(result["ok"], (question, result))
            self.assertEqual(result["question_class"], "total", question)
            self.assertNotIn("GROUP BY", result["sql"], question)


class AQuantifierNeverNamesADimension(unittest.TestCase):
    def test_total_does_not_reach_country_through_count(self):
        for question in ("total deaths by", "the total number by",
                         "overall amount by", "average by"):
            self.assertIsNone(engine._match_any_dim(question, DIMS), question)

    def test_a_dimension_the_question_does_name_is_still_found(self):
        for question, col in (("deaths by country", "country"),
                              ("which countries lead", "country"),
                              ("total deaths by region", "region"),
                              ("the highest region", "region")):
            self.assertEqual(engine._match_any_dim(question, DIMS), col, question)

    def test_a_dimension_named_by_a_quantifier_is_still_reachable_explicitly(self):
        """The exclusion is for the loose search only. A dataset may genuinely
        have a dimension called "total", and "deaths by total" names it."""
        dims = [{"column_name": "total"}, {"column_name": "country"}]
        self.assertEqual(engine._group_by_candidates("deaths by total", dims), ["total"])


class RoutingDiscountsTheSameWordsTheMatcherDoes(unittest.TestCase):
    ARTIFACTS = {"rows_objects": [
        {"dataset_id": "sessions", "manifest": json.dumps({
            "name": "Session counts",
            "metrics": [{"name": "Number of sessions"}, {"name": "Average duration"}],
            "columns": [{"name": "surface"}]})},
        {"dataset_id": "people", "manifest": json.dumps({
            "name": "Users",
            "metrics": [{"name": "Active users"}],
            "columns": [{"name": "plan"}]})},
        {"dataset_id": "totals", "manifest": json.dumps({
            "name": "Totals",
            "metrics": [{"name": "Totals"}],
            "columns": [{"name": "k"}]})},
    ]}

    def route(self, question):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine, "meta", SimpleNamespace(query=lambda *a, **k: self.ARTIFACTS)), \
             patch.object(engine, "_value_dataset_hits", lambda toks: {}):
            return engine.route(question)

    def test_a_quantifier_alone_does_not_route(self):
        """"total number of users" names users. Before, "number" scored a x3
        metric hit against "Number of sessions" and tied with it."""
        for question in ("total number of users",
                         "the average number of users",
                         "overall amount of users"):
            self.assertEqual([r["dataset_id"] for r in self.route(question)], ["people"], question)

    def test_a_quantifier_does_not_route_by_its_stem_either(self):
        """Subtracting the words alone left "Totals" matching "total" on the
        stem pass that runs next."""
        self.assertEqual([r["dataset_id"] for r in self.route("total number of all the things")], [])

    def test_a_question_that_really_is_about_those_measures_still_routes(self):
        self.assertEqual([r["dataset_id"] for r in self.route("average duration by surface")],
                         ["sessions"])
        self.assertEqual([r["dataset_id"] for r in self.route("sessions by surface")],
                         ["sessions"])


if __name__ == "__main__":
    unittest.main()
