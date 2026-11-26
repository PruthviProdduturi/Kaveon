"""A created dataset compiles its DLM without anyone pressing a button.

A dataset with no artifact answers nothing on the DLM path, which is the
product's primary path, and the context banner reads "No context compiled
yet". Nothing was compiling it: `create_dataset` returned the row and stopped,
and the only caller of the generate endpoint was the bulk repair action in
Settings. So an ordinary create left the dataset mute until someone noticed.

What is pinned here is the whole contract, not just that the call happens:
the compile is scheduled rather than inline, it carries the caller's identity,
and a compile that fails or declines leaves the created dataset standing.
"""

import unittest
from unittest.mock import patch

from fastapi import BackgroundTasks

from middleware.auth import UserContext
from models.datasets import DatasetCreate
from routers import datasets as datasets_router

ANALYST = UserContext("analyst@example.com", "Analyst")

CREATED = {
    "id": "412", "name": "Energy mix", "created_by": "analyst@example.com",
    "database_name": "OpenSource", "schema_name": "public",
    "table_name": "energy_mix", "visibility": "internal",
    "columns": [], "dimensions": [], "metrics": [],
}


def _create(created=CREATED):
    """Run the create route against a stubbed store, returning its tasks."""
    background = BackgroundTasks()
    body = DatasetCreate(name="Energy mix", database_name="OpenSource",
                         schema_name="public", table_name="energy_mix")
    with patch.object(datasets_router.svc, "create_dataset", return_value=created), \
         patch.object(datasets_router.engine_datasets, "apply_binding",
                      side_effect=lambda payload, *a, **k: payload):
        result = datasets_router.create_dataset(body, ANALYST, background)
    return result, background


class DatasetCreateCompilesItsDlm(unittest.TestCase):
    def test_a_created_dataset_schedules_its_compile(self):
        result, background = _create()
        self.assertEqual(result["id"], "412")
        self.assertEqual(len(background.tasks), 1,
                         "a create should schedule exactly one compile")
        self.assertIs(background.tasks[0].func, datasets_router._compile_dlm)

    def test_the_compile_carries_the_caller_identity(self):
        """The artifact is written as the person who created the dataset."""
        _, background = _create()
        self.assertEqual(background.tasks[0].args,
                         ("412", "analyst@example.com", "Analyst"))

    def test_the_compile_runs_after_the_response_not_during_it(self):
        """Profiling reads the source. A create that blocked on it would be
        slowest exactly where the DLM matters most -- a large table."""
        with patch.object(datasets_router, "_compile_dlm") as compile_now:
            _create()
            compile_now.assert_not_called()

    def test_a_dataset_with_no_id_schedules_nothing(self):
        """Nothing to compile, and no task carrying a null id."""
        _, background = _create(created={"name": "no id"})
        self.assertEqual(background.tasks, [])

    def test_a_non_dict_result_schedules_nothing(self):
        _, background = _create(created=None)
        self.assertEqual(background.tasks, [])

    def test_a_caller_that_passes_no_background_still_creates(self):
        """`create_dataset(body, ctx)` is how the route is called directly, by
        tests and by internal callers. Taking the second position for the
        background parameter bound a UserContext to it and broke every one of
        them, so it sits last and optional; a caller that omits it gets its
        dataset and no compile, rather than an AttributeError."""
        body = DatasetCreate(name="Energy mix", database_name="OpenSource",
                             schema_name="public", table_name="energy_mix")
        with patch.object(datasets_router.svc, "create_dataset", return_value=CREATED), \
             patch.object(datasets_router.engine_datasets, "apply_binding",
                          side_effect=lambda payload, *a, **k: payload):
            result = datasets_router.create_dataset(body, ANALYST)
        self.assertEqual(result["id"], "412")


class AFailedCompileLeavesTheDatasetStanding(unittest.TestCase):
    """The create already answered 201. A task that raised would contradict a
    response the client has, and lose a dataset that is valid either way."""

    def test_a_raising_generate_is_swallowed_and_logged(self):
        with patch("dlm.engine.generate_dlm", side_effect=RuntimeError("profiler down")), \
             self.assertLogs("routers.datasets", level="WARNING") as logged:
            datasets_router._compile_dlm("412", "analyst@example.com", "Analyst")
        self.assertIn("dlm_autocompile_failed", "".join(logged.output))
        self.assertIn("RuntimeError", "".join(logged.output))

    def test_a_declined_generate_is_logged_with_its_reason(self):
        """`generate_dlm` reports a refusal in its result rather than raising;
        an unqueryable dataset should still say why in the log."""
        declined = {"ok": False, "reason": "dataset_not_found"}
        with patch("dlm.engine.generate_dlm", return_value=declined), \
             self.assertLogs("routers.datasets", level="WARNING") as logged:
            datasets_router._compile_dlm("412", "analyst@example.com", "Analyst")
        self.assertIn("dlm_autocompile_declined", "".join(logged.output))
        self.assertIn("dataset_not_found", "".join(logged.output))

    def test_a_successful_compile_logs_nothing(self):
        """The quiet path stays quiet: no warning for the ordinary case."""
        with patch("dlm.engine.generate_dlm", return_value={"ok": True}):
            with self.assertRaises(AssertionError):
                with self.assertLogs("routers.datasets", level="WARNING"):
                    datasets_router._compile_dlm("412", "analyst@example.com", "Analyst")

    def test_the_compile_asks_for_the_dataset_it_was_given(self):
        with patch("dlm.engine.generate_dlm", return_value={"ok": True}) as generate:
            datasets_router._compile_dlm("412", "analyst@example.com", "Analyst")
        generate.assert_called_once_with("412", actor="analyst@example.com", role="Analyst")


if __name__ == "__main__":
    unittest.main()
