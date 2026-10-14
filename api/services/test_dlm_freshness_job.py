"""The scheduled recompile of stale DLM context.

A dataset's row count, date range and answerable values are recorded when its
context is compiled, not measured when they are read — so a table taking a
daily append keeps reporting yesterday's numbers until something recompiles
it. `check_freshness` detects that; nothing acted on it unattended, because
the in-process sweep loop is disabled whenever the PostgreSQL-free runtime is
requested and has no identity to read compiled artifacts with anyway.

These cases pin what the job does with what it finds: every stale dataset
recompiled, every fresh one left alone, one failure not stranding the rest,
and the work finished before the process exits.
"""

import sys
import unittest
from unittest.mock import patch

from services import dlm_freshness_job as job


class _Engine:
    """Stands in for `dlm.engine`, recording what the sweep asked it to do."""

    class _RetirementServingState:
        def __init__(self, actor, role):
            self.actor, self.role = actor, role

    class _ContextVar:
        def __init__(self):
            self.value = None

        def set(self, value):
            previous, self.value = self.value, value
            return previous

        def reset(self, token):
            self.value = token

        def get(self):
            return self.value

    def __init__(self, artifacts, freshness, fail=()):
        self.artifacts = artifacts
        self.freshness = freshness
        self.fail = set(fail)
        self.generated = []
        self.identity_during_listing = None
        self._RETIREMENT_SERVING = self._ContextVar()

    def _serving_artifacts(self):
        state = self._RETIREMENT_SERVING.get()
        self.identity_during_listing = (state.actor, state.role) if state else None
        return self.artifacts

    def check_freshness(self, dataset_id):
        return self.freshness[dataset_id]

    def generate_dlm(self, dataset_id, force=False, actor=None, role="Viewer"):
        self.generated.append({"dataset_id": dataset_id, "force": force,
                               "actor": actor, "role": role})
        if dataset_id in self.fail:
            raise RuntimeError("compile exceeded its budget")
        return {"status": "ready"}


FRESH = {"fresh": True, "recommendation": "none", "score": 1.0}
STALE = {"fresh": False, "recommendation": "rebuild", "score": 0.1,
         "reason": "source version changed"}


class SweepRecompilesWhatHasChanged(unittest.TestCase):
    def _run(self, engine, actor="sweep@kaveon.io"):
        module = type(sys)("dlm")
        module.engine = engine
        with patch.dict(sys.modules, {"dlm": module, "dlm.engine": engine}):
            return job.sweep(actor)

    def test_a_dataset_whose_source_has_moved_is_recompiled(self):
        engine = _Engine([{"dataset_id": "136"}, {"dataset_id": "144"}],
                         {"136": STALE, "144": FRESH})
        result = self._run(engine)

        self.assertEqual(result["checked"], 2)
        self.assertEqual(result["stale"], ["136"])
        self.assertEqual(result["rebuilt"], ["136"])
        # A fresh dataset is not recompiled: the whole point of comparing the
        # recorded source identity is to not pay for a rebuild that would
        # produce the same context.
        self.assertEqual([g["dataset_id"] for g in engine.generated], ["136"])

    def test_the_recompile_is_forced_and_runs_as_the_sweep_principal(self):
        engine = _Engine([{"dataset_id": "136"}], {"136": STALE})
        self._run(engine, actor="sweep@kaveon.io")
        self.assertEqual(engine.generated, [{"dataset_id": "136", "force": True,
                                             "actor": "sweep@kaveon.io", "role": "Admin"}])
        # Compiled context is owned content, so even listing it needs an
        # identity. A sweep without one checked nothing at all.
        self.assertEqual(engine.identity_during_listing, ("sweep@kaveon.io", "Admin"))

    def test_one_dataset_that_cannot_compile_does_not_strand_the_others(self):
        engine = _Engine([{"dataset_id": "136"}, {"dataset_id": "144"}],
                         {"136": STALE, "144": STALE}, fail={"136"})
        result = self._run(engine)

        self.assertEqual(result["rebuilt"], ["144"])
        self.assertEqual([entry["dataset_id"] for entry in result["failed"]], ["136"])
        # Both were attempted. A failure costs that dataset its freshness, not
        # every dataset behind it in the list.
        self.assertEqual([g["dataset_id"] for g in engine.generated], ["136", "144"])

    def test_an_unreadable_freshness_check_is_skipped_not_fatal(self):
        # 136 has no freshness entry, so checking it raises.
        engine = _Engine([{"dataset_id": "136"}, {"dataset_id": "144"}], {"144": STALE})
        result = self._run(engine)
        self.assertEqual(result["checked"], 2)
        self.assertEqual(result["stale"], ["144"])
        self.assertEqual(result["rebuilt"], ["144"])

    def test_nothing_stale_recompiles_nothing(self):
        engine = _Engine([{"dataset_id": "136"}], {"136": FRESH})
        result = self._run(engine)
        self.assertEqual(result, {"checked": 1, "stale": [], "rebuilt": [], "failed": []})
        self.assertEqual(engine.generated, [])


class SweepRefusesWithoutAPrincipal(unittest.TestCase):
    def test_no_configured_principal_is_refused_rather_than_swept_as_nobody(self):
        with patch.dict("os.environ", {"KAVEON_DLM_SWEEP_ACTOR": ""}):
            self.assertIsNone(job.sweep_identity())
        with patch.object(job, "sweep_identity", return_value=None), \
             patch.object(job, "sweep") as swept:
            self.assertEqual(job.main(), 2)
        # Silently checking nothing is the failure this replaces: the
        # in-process loop did exactly that and reported success.
        swept.assert_not_called()

    def test_a_configured_principal_is_what_the_sweep_runs_as(self):
        with patch.dict("os.environ", {"KAVEON_DLM_SWEEP_ACTOR": " sweep@kaveon.io "}):
            self.assertEqual(job.sweep_identity(), "sweep@kaveon.io")


class ExitStatusSaysWhatHappened(unittest.TestCase):
    def test_a_run_with_nothing_to_do_succeeds(self):
        with patch.object(job, "sweep_identity", return_value="sweep@kaveon.io"), \
             patch.object(job, "sweep", return_value={"checked": 9, "stale": [],
                                                      "rebuilt": [], "failed": []}):
            self.assertEqual(job.main(), 0)

    def test_a_failed_recompile_fails_the_run(self):
        with patch.object(job, "sweep_identity", return_value="sweep@kaveon.io"), \
             patch.object(job, "sweep", return_value={
                 "checked": 9, "stale": ["136"], "rebuilt": [],
                 "failed": [{"dataset_id": "136", "error": "out of memory"}]}):
            # A permanently stale dataset must not be invisible behind a zero
            # exit: the CronJob's history is where this is noticed.
            self.assertEqual(job.main(), 1)


class DiscoveryIsOffUnlessAsked(unittest.TestCase):
    """A scheduled job that registers tables into the product catalog on its
    own is the behaviour that swept benchmark scratch in once already, so the
    default is to look at nothing, and the middle setting looks without
    writing."""

    def test_the_default_does_not_look(self):
        with patch.dict("os.environ", {}, clear=False):
            import os
            os.environ.pop("KAVEON_DLM_DISCOVER_TABLES", None)
            self.assertEqual(job.discovery_mode(), "off")
            self.assertIsNone(job.discover("a@b.c", "off"))

    def test_report_looks_without_writing(self):
        with patch.dict("os.environ", {"KAVEON_DLM_DISCOVER_TABLES": "report"}):
            self.assertEqual(job.discovery_mode(), "report")
        with patch("services.lake_discovery.discover", return_value={"found": []}) as found:
            job.discover("a@b.c", "report")
        self.assertFalse(found.call_args.kwargs["register"])

    def test_register_writes(self):
        for raw in ("register", "true", "1", "yes"):
            with patch.dict("os.environ", {"KAVEON_DLM_DISCOVER_TABLES": raw}):
                self.assertEqual(job.discovery_mode(), "register", raw)
        with patch("services.lake_discovery.discover", return_value={"found": []}) as found:
            job.discover("a@b.c", "register")
        self.assertTrue(found.call_args.kwargs["register"])

    def test_a_discovery_failure_does_not_stop_the_sweep(self):
        """Discovery is an addition to this job; it must not be able to stop
        the thing the job already did."""
        with patch("services.lake_discovery.discover",
                   side_effect=RuntimeError("listing refused")):
            outcome = job.discover("a@b.c", "register")
        self.assertIn("listing refused", outcome["failed"])

        with patch.object(job, "sweep_identity", return_value="sweep@kaveon.io"),              patch.object(job, "discovery_mode", return_value="register"),              patch("services.lake_discovery.discover",
                   side_effect=RuntimeError("listing refused")),              patch.object(job, "sweep", return_value={"checked": 1, "stale": [],
                                                      "rebuilt": [], "failed": []}) as swept:
            self.assertEqual(job.main(), 0)
        swept.assert_called_once()

    def test_discovery_runs_before_the_sweep(self):
        """So a table registered now is compiled in this run rather than
        waiting an hour for the next one."""
        order = []
        with patch.object(job, "sweep_identity", return_value="sweep@kaveon.io"),              patch.object(job, "discovery_mode", return_value="register"),              patch.object(job, "discover",
                          side_effect=lambda *a, **k: order.append("discover") or
                          {"registered": [], "found": [], "undecidable": [], "unreadable": []}),              patch.object(job, "sweep",
                          side_effect=lambda *a, **k: order.append("sweep") or
                          {"checked": 1, "stale": [], "rebuilt": [], "failed": []}):
            job.main()
        self.assertEqual(order, ["discover", "sweep"])


if __name__ == "__main__":
    unittest.main()
