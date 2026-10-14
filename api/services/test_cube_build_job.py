"""The cube build job refuses to guess, and says what happened.

The behaviours pinned here are the ones that cost real time when they were
absent: a build that runs as an ambient administrator, a build that picks its
own table, and a build that scans for tens of minutes before the Engine
mentions the table has no shape to build over.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import cube_build_job as job


SHAPED = {
    "id": "table:KaveonTelemetry:events:enriched",
    "shape": {"dimensions": [{"name": n} for n in "abcdefghij"],
              "measures": [{"column": "actions"}]},
}


class TargetTests(unittest.TestCase):
    def test_a_three_part_name_is_read(self):
        with patch.dict("os.environ", {"KAVEON_CUBE_TARGET": "Cat.sch.tbl"}, clear=False):
            self.assertEqual(job.target(), ("Cat", "sch", "tbl"))

    def test_anything_else_is_refused_rather_than_guessed(self):
        for raw in ("", "   ", "cat", "cat.sch", "cat.sch.tbl.extra", "cat..tbl"):
            with patch.dict("os.environ", {"KAVEON_CUBE_TARGET": raw}, clear=False):
                self.assertIsNone(job.target(), raw)


class RefusalTests(unittest.TestCase):
    def test_no_actor_refuses_and_does_not_build(self):
        with patch.dict("os.environ", {"KAVEON_CUBE_ACTOR": "",
                                       "KAVEON_CUBE_TARGET": "c.s.t"}, clear=False), \
             patch.object(job, "build") as build:
            self.assertEqual(job.main(), 2)
        build.assert_not_called()

    def test_no_target_refuses_and_does_not_build(self):
        with patch.dict("os.environ", {"KAVEON_CUBE_ACTOR": "a@b.c",
                                       "KAVEON_CUBE_TARGET": ""}, clear=False), \
             patch.object(job, "build") as build:
            self.assertEqual(job.main(), 2)
        build.assert_not_called()

    def test_a_table_with_no_shape_is_refused_before_any_scan(self):
        """The Engine says this too, but only after the caller has committed to
        the wait. A ten-axis build is tens of minutes; the refusal is free."""
        with patch("services.engine_bridge.table_definition_by_id",
                   return_value={"id": "t", "shape": None}), \
             patch("services.engine_bridge.analyze_table") as analyze:
            outcome = job.build("c", "s", "t", "a@b.c")
        self.assertFalse(outcome["ok"])
        self.assertIn("no shape", outcome["reason"])
        analyze.assert_not_called()

    def test_a_table_the_catalog_does_not_hold_is_named(self):
        with patch("services.engine_bridge.table_definition_by_id", return_value=None), \
             patch("services.engine_bridge.analyze_table") as analyze:
            outcome = job.build("c", "s", "t", "a@b.c")
        self.assertFalse(outcome["ok"])
        self.assertIn("table:c:s:t", outcome["reason"])
        analyze.assert_not_called()


class BuildTests(unittest.TestCase):
    def test_a_shaped_table_builds_with_cube_and_the_caller_as_admin(self):
        with patch("services.engine_bridge.table_definition_by_id", return_value=SHAPED), \
             patch("services.engine_bridge.analyze_table",
                   return_value=("ANALYZE x", {"columns": [{"name": "cells"}],
                                               "data": [[4096]]})) as analyze:
            outcome = job.build("KaveonTelemetry", "events", "enriched", "a@b.c")
        self.assertTrue(outcome["ok"])
        self.assertEqual(outcome["axes"], 10)
        self.assertEqual(outcome["summary"], {"cells": 4096})
        args, kwargs = analyze.call_args
        self.assertEqual(args[:5], ("KaveonTelemetry", "events", "enriched", "a@b.c", "Admin"))
        self.assertTrue(kwargs["cube"])

    def test_an_engine_refusal_is_reported_rather_than_raised(self):
        """The Engine's own message is the thing an operator has to act on, so
        it reaches the job's output instead of a traceback and an exit code."""
        with patch.dict("os.environ", {"KAVEON_CUBE_ACTOR": "a@b.c",
                                       "KAVEON_CUBE_TARGET": "c.s.t"}, clear=False), \
             patch.object(job, "build", side_effect=RuntimeError("admission rejected")):
            self.assertEqual(job.main(), 1)

    def test_a_successful_run_exits_zero(self):
        with patch.dict("os.environ", {"KAVEON_CUBE_ACTOR": "a@b.c",
                                       "KAVEON_CUBE_TARGET": "c.s.t"}, clear=False), \
             patch.object(job, "build", return_value={"ok": True, "seconds": 1.0,
                                                      "axes": 10, "summary": {}}):
            self.assertEqual(job.main(), 0)


class TaggingTests(unittest.TestCase):
    def test_the_build_is_tagged_maintenance_by_the_bridge(self):
        """The job does not stamp the tag itself; `analyze_table` does, which is
        what keeps the interactive path and this one from drifting apart."""
        from services import engine_bridge
        captured = {}

        def fake_send(method, path, token, actor, **kwargs):
            captured.update(kwargs.get("payload") or {})
            return SimpleNamespace(status_code=200, is_success=True,
                                   json=lambda: {"data": [[]], "columns": []})

        with patch("services.engine_bridge.table_definition_by_id", return_value=SHAPED), \
             patch.object(engine_bridge, "_send", side_effect=fake_send), \
             patch.object(engine_bridge, "_response_json",
                          return_value={"data": [[]], "columns": []}):
            job.build("KaveonTelemetry", "events", "enriched", "a@b.c")

        self.assertIn(engine_bridge.MAINTENANCE_CLIENT_TAG, captured["client_tags"])


if __name__ == "__main__":
    unittest.main()
