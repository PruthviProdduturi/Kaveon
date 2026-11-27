"""Freshness could be acted on but not looked at.

Per dataset there was always `GET /datasets/{id}/freshness`, and on the ask
path a stale context declines itself and schedules its own rebuild. Across the
fleet there was only `POST /dlm/sweep`, which rebuilds everything it finds
stale -- so an operator asking "what is stale right now?" could not get an
answer without changing the answer, and between sweeps there was nothing to
look at at all.

`freshness_status` is that read. The tests that matter here are the two it
would be easy to get wrong: that it triggers nothing, and that the fleet is
ordered so the rows needing attention are the ones at the top.
"""

import json
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from dlm import engine


def _artifact(dataset_id, fact_table="events", status="ready"):
    return {"dataset_id": dataset_id, "status": status,
            "manifest": json.dumps({"fact_table": fact_table, "schema": "public"})}


STALE = {"fresh": False, "score": 0.2, "recommendation": "rebuild",
         "data_modified": True, "computed_at": "2026-11-20T00:00:00",
         "signal": "engine_source_version"}
FRESH = {"fresh": True, "score": 0.9, "recommendation": "use_context",
         "data_modified": False, "computed_at": "2026-11-26T00:00:00",
         "signal": "engine_source_version"}
ABSENT = {"fresh": False, "score": 0.0, "recommendation": "no_context",
          "data_modified": False, "computed_at": None,
          "signal": "engine_source_version"}


def _status(artifacts, freshness, in_flight=frozenset(), actor="admin@example.test"):
    """Run the status read over a fixed fleet, with rebuilds observable."""
    triggered = []
    with patch.object(engine, "_serving_artifacts", return_value=artifacts), \
         patch.object(engine, "check_freshness", lambda did: freshness[str(did)]), \
         patch.object(engine, "_REBUILD_ACTIVE", set(in_flight)), \
         patch.object(engine, "_trigger_background_rebuild",
                      side_effect=lambda did, actor=None: triggered.append(did) or True):
        result = engine.freshness_status(actor, "Admin")
    return result, triggered


class LookingDoesNotAct(unittest.TestCase):
    """The property the endpoint exists for. A status page that rebuilt what it
    displayed would make every refresh a fleet-wide recompile."""

    FLEET = [_artifact("7"), _artifact("9", "orders")]
    ALL_STALE = {"7": STALE, "9": STALE}

    def test_a_fleet_of_nothing_but_stale_contexts_triggers_no_rebuild(self):
        result, triggered = _status(self.FLEET, self.ALL_STALE)
        self.assertEqual(result["stale"], 2)
        self.assertEqual(triggered, [], "the status read rebuilt what it reported")

    def test_the_sweep_over_the_same_fleet_does_trigger(self):
        """Teeth for the test above: the assertion is only meaningful because
        this input is one the sweep acts on."""
        triggered = []
        with patch.object(engine, "_serving_artifacts", return_value=self.FLEET), \
             patch.object(engine, "check_freshness", lambda did: STALE), \
             patch.object(engine, "_trigger_background_rebuild",
                          side_effect=lambda did, actor=None: triggered.append(did) or True):
            engine.freshness_sweep("admin@example.test", "Admin")
        self.assertEqual(sorted(triggered), ["7", "9"])


class TheWorstRowsComeFirst(unittest.TestCase):
    def test_uncompiled_then_stale_then_serving(self):
        fleet = [_artifact("1"), _artifact("2"), _artifact("3")]
        result, _ = _status(fleet, {"1": FRESH, "2": ABSENT, "3": STALE})
        self.assertEqual([row["dataset_id"] for row in result["datasets"]],
                         ["2", "3", "1"])

    def test_within_a_recommendation_the_lowest_score_is_first(self):
        worse = {**STALE, "score": 0.05}
        result, _ = _status([_artifact("1"), _artifact("2")],
                            {"1": STALE, "2": worse})
        self.assertEqual([row["dataset_id"] for row in result["datasets"]],
                         ["2", "1"])


class ARebuildInFlightIsPartOfTheStatus(unittest.TestCase):
    """Stale with a rebuild running needs no human; stale with none needs one
    now. The recommendation reads `rebuild` either way, so the difference has
    to come from somewhere else."""

    def test_a_dataset_being_rebuilt_is_marked_and_counted(self):
        result, _ = _status([_artifact("7"), _artifact("9")],
                            {"7": STALE, "9": STALE}, in_flight={"7"})
        by_id = {row["dataset_id"]: row for row in result["datasets"]}
        self.assertTrue(by_id["7"]["rebuilding"])
        self.assertFalse(by_id["9"]["rebuilding"])
        self.assertEqual((result["stale"], result["rebuilding"]), (2, 1))

    def test_a_quiet_fleet_reports_no_rebuilds(self):
        result, _ = _status([_artifact("7")], {"7": FRESH})
        self.assertEqual(result["rebuilding"], 0)


class TheCountsSummariseTheRows(unittest.TestCase):
    def test_each_count_matches_what_the_rows_say(self):
        fleet = [_artifact("1"), _artifact("2"), _artifact("3"), _artifact("4")]
        result, _ = _status(fleet, {"1": FRESH, "2": STALE, "3": STALE, "4": ABSENT})
        self.assertEqual(result["checked"], 4)
        self.assertEqual(result["stale"], 2)
        self.assertEqual(result["uncompiled"], 1)
        self.assertEqual(len(result["datasets"]), 4)

    def test_the_fact_table_is_reported_so_the_rows_can_be_told_apart(self):
        result, _ = _status([_artifact("7", "orders")], {"7": FRESH})
        self.assertEqual(result["datasets"][0]["fact_table"], "orders")

    def test_an_artifact_with_no_dataset_id_is_skipped_not_counted(self):
        result, _ = _status([_artifact("7"), {"status": "ready"}], {"7": FRESH})
        self.assertEqual(result["checked"], 1)


class AnUnattendedReadSaysSo(unittest.TestCase):
    def test_no_identity_reports_the_reason_rather_than_an_empty_fleet(self):
        """Zero checked with no reason reads as "everything is fresh". The read
        needs a caller to resolve product state as, so it says that instead --
        the same contract the sweep has."""
        with patch.object(engine, "_serving_artifacts") as artifacts:
            result = engine.freshness_status()
        self.assertEqual(result["checked"], 0)
        self.assertIn("identity", result["skipped"])
        artifacts.assert_not_called()


if __name__ == "__main__":
    unittest.main()
