"""`engine.coverage` — what the home page's available-context banner costs.

The banner names each compiled dataset, its date range, its row count and a few
indexed values. It used to pay, per artifact, a sample read, a count read, a
full dataset-document read and a COUNT(*) per table through the Engine, writing
the result back — and when a source could not answer that count nothing was
written, so the whole attempt repeated on every load of the page. These cases
pin it to reads: one for the artifacts, one for their values, and no write.

It then still cost 1.45s on a repeat load, because establishing whether
anything had changed meant three catalog listings and the Engine answers any
product listing in about half a second (defect D-3). The later cases pin the
repeat load to *no* Engine round trip at all, with the recheck behind the
response and the staleness bounded.
"""

import contextlib
import threading
import unittest
from unittest.mock import Mock, patch

from dlm import engine
from services import dlm_compiled_artifact as _compiled
from services import postgresql_retirement_runtime as _runtime
from services import revalidating_cache


class _Clock:
    """Time this test moves by hand, so no case depends on how long it ran."""

    def __init__(self):
        self.now = 1_000.0

    def advance(self, seconds):
        self.now += seconds

    def __call__(self):
        return self.now


class _Spawner:
    """Runs work behind the response on a real thread, and lets a case wait."""

    def __init__(self):
        self.threads = []

    def __call__(self, work, name):
        thread = threading.Thread(target=work, name=name, daemon=True)
        self.threads.append(thread)
        thread.start()

    def join(self):
        for thread in self.threads:
            thread.join(timeout=10)
            assert not thread.is_alive(), f"{thread.name} did not finish"
        self.threads = []


ARTIFACTS = [
    {"dataset_id": "136", "status": "ready", "built_at": "2026-10-01T00:00:00Z",
     "manifest": {"name": "AI Benchmark Scores", "date_column": "release_date",
                  "columns": [{"name": "model_name", "is_dimension": True},
                              {"name": "score", "is_metric": True}],
                  "metrics": [{"name": "Mean score"}]},
     # No exact count was ever established for this artifact.
     "stats_rollup": {"watermark": {"row_count": 4200},
                      "date_range": {"min": "2025-01-01", "max": "2026-09-30"}}},
    {"dataset_id": "144", "status": "ready", "built_at": "2026-09-01T00:00:00Z",
     "manifest": {"name": "Climate and Energy", "columns": [], "metrics": []},
     "stats_rollup": {"row_counts": {"energy_mix": 91000},
                      "row_count_source": "kaveon_engine_exact"}},
]

VALUE_INDEX = [
    {"dataset_id": "136", "element_key": "benchmark_scores.model_name",
     "value_text": "Claude", "freq": 90},
    {"dataset_id": "136", "element_key": "benchmark_scores.model_name",
     "value_text": "Gemini", "freq": 40},
    {"dataset_id": "136", "element_key": "benchmark_scores.model_name",
     "value_text": "Claude", "freq": 10},
]


class _Reads:
    """Stands in for the metadata DB and records every statement it is given."""

    def __init__(self):
        self.statements = []

    def query(self, sql, params=None):
        self.statements.append(sql)
        if "dlm_artifact" in sql:
            return {"rows_objects": ARTIFACTS}
        if "dlm_value_index" in sql:
            return {"rows_objects": VALUE_INDEX}
        raise AssertionError(f"unexpected read: {sql}")


class ContextBannerCoverage(unittest.TestCase):
    def _run(self):
        reads = _Reads()
        with patch.object(engine, "ensure_tables"), \
             patch.object(engine.meta, "query", side_effect=reads.query), \
             patch.object(engine.meta, "query_one") as query_one, \
             patch.object(engine.meta, "execute") as execute, \
             patch.object(engine.datasets_svc, "get_dataset_by_id") as dataset_read, \
             patch.object(engine, "_native_row_counts") as native_counts:
            rows = engine.coverage()
        return rows, reads, query_one, execute, dataset_read, native_counts

    def test_the_banner_reads_the_artifacts_and_their_values_and_nothing_else(self):
        rows, reads, query_one, execute, dataset_read, native_counts = self._run()

        # One artifact read and one value-index read, regardless of how many
        # datasets the banner covers.
        self.assertEqual(len(reads.statements), 2)
        self.assertIn("dlm_artifact", reads.statements[0])
        self.assertIn("dlm_value_index", reads.statements[1])
        self.assertIn("dataset_id IN (@param0, @param1)", reads.statements[1])
        query_one.assert_not_called()

        # Rendering a banner neither reads a dataset document, nor measures a
        # row count through the Engine, nor writes anything back.
        dataset_read.assert_not_called()
        native_counts.assert_not_called()
        execute.assert_not_called()

        self.assertEqual([row["dataset_id"] for row in rows], ["136", "144"])

    def test_a_row_count_is_reported_as_recorded_and_never_manufactured(self):
        rows, *_ = self._run()
        by_id = {row["dataset_id"]: row for row in rows}
        # Exact where the artifact recorded one.
        self.assertEqual(by_id["144"]["row_count"], 91000)
        self.assertEqual(by_id["144"]["row_count_source"], "kaveon_engine_exact")
        # The watermark the artifact holds where it did not, and no claim that
        # the number is exact.
        self.assertEqual(by_id["136"]["row_count"], 4200)
        self.assertIsNone(by_id["136"]["row_count_source"])

    def test_values_and_dimension_samples_come_from_the_one_bulk_read(self):
        rows, *_ = self._run()
        by_id = {row["dataset_id"]: row for row in rows}
        self.assertEqual(by_id["136"]["values_indexed"], 3)
        self.assertEqual(by_id["136"]["dimensions"],
                         [{"column": "model_name", "values": ["Claude", "Gemini"]}])
        self.assertEqual(by_id["136"]["metrics"], ["Mean score"])
        # An artifact with no indexed value reports none rather than borrowing
        # another artifact's.
        self.assertEqual(by_id["144"]["values_indexed"], 0)
        self.assertEqual(by_id["144"]["dimensions"], [])


class BulkValueIndex(unittest.TestCase):
    def test_samples_are_capped_per_column_and_deduplicated(self):
        rows = [{"dataset_id": "1", "element_key": "t.c", "value_text": f"v{i}", "freq": 100 - i}
                for i in range(10)]
        rows.append({"dataset_id": "1", "element_key": "t.c", "value_text": "v0", "freq": 1})
        with patch.object(engine.meta, "query", return_value={"rows_objects": rows}):
            samples, counts = engine._bulk_value_index(["1"], serving=False, per_col=3)
        self.assertEqual(samples["1"]["c"], ["v0", "v1", "v2"])
        self.assertEqual(counts["1"], 11)

    def test_no_artifact_reads_nothing(self):
        with patch.object(engine.meta, "query") as query:
            self.assertEqual(engine._bulk_value_index([], serving=False), ({}, {}))
        query.assert_not_called()

    def test_a_failed_read_leaves_the_banner_without_values_rather_than_failing(self):
        with patch.object(engine.meta, "query", side_effect=RuntimeError("index unavailable")):
            samples, counts = engine._bulk_value_index(["1"], serving=False)
        self.assertEqual((samples, counts), ({}, {}))


class TheBannerIsServedFromMemoryWithNoEngineRoundTrip(unittest.TestCase):
    """A repeat load of the banner must not ask the Engine anything at all.

    The previous cache was keyed on a digest of the dataset, definition and run
    records, which is exact — but taking that digest means three catalog
    listings, and the Engine answers any product listing in about half a second
    regardless of how few records it returns (defect D-3). So a cache *hit*
    still cost ~1.4s and saved only the artifact reads: measured in the
    cluster, 2.82s cold against 1.45s warm.

    The answer is now served from memory first and the digest rechecked behind
    the response. These cases pin that down: a served hit makes no listing and
    no artifact read, it answers even when both would fail, a recheck is
    started once however many readers arrive, and the staleness is bounded —
    a write this process performs is reflected immediately, and an answer
    whose recheck keeps failing stops being served.
    """

    def setUp(self):
        self.clock = _Clock()
        self.spawner = _Spawner()
        self.cache = revalidating_cache.RevalidatingCache(
            "test-dlm-coverage", fresh_for=engine.COVERAGE_FRESH_FOR,
            max_served_age=engine.COVERAGE_MAX_SERVED_AGE,
            clock=self.clock, spawn=self.spawner)
        patcher = patch.object(engine, "_COVERAGE_CACHE", self.cache)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self.spawner.join)

    @contextlib.contextmanager
    def _engine(self, *, identity="digest-a", listing=None, artifacts=ARTIFACTS):
        """Stand in for every Engine round trip the banner could make.

        `listing` is the three catalog listings that establish the digest and
        `artifacts` the compiled-artifact reads behind the answer — the two
        things a served hit must not do.
        """
        listed = Mock(side_effect=listing or (lambda _actor: object()))
        read = (Mock(side_effect=artifacts) if callable(artifacts)
                else Mock(return_value=artifacts))
        with patch.object(engine, "ensure_tables"), \
             patch.object(_runtime, "requested", return_value=True), \
             patch.object(_compiled, "load_bulk_context", listed), \
             patch.object(_compiled, "context_identity", return_value=identity), \
             patch.object(engine, "_serving_artifacts", read), \
             patch.object(engine, "_bulk_value_index", return_value=({}, {})):
            yield listed, read

    def _coverage(self, actor="owner", role="Admin", **engine_state):
        """Render the banner as `actor`, and let the recheck behind it finish.

        Waiting for the recheck is what makes these cases deterministic — what
        is asserted is which reads happened, never how long anything took.
        """
        with self._engine(**engine_state) as (listed, read):
            rows = engine.coverage(actor, role)
            self.spawner.join()
        return rows, listed, read

    def _unavailable(self, _actor=None):
        raise RuntimeError("the Engine is not answering")

    def test_a_repeat_load_asks_the_engine_for_nothing(self):
        first, listed, read = self._coverage()
        listed.assert_called_once()
        read.assert_called_once()

        second, listed, read = self._coverage()
        # Zero round trips: neither the listings that establish the digest nor
        # the artifacts behind the answer.
        listed.assert_not_called()
        read.assert_not_called()
        self.assertEqual(second, first)

    def test_a_repeat_load_answers_while_the_engine_is_refusing_both_reads(self):
        first, *_ = self._coverage()
        # Would fail on any implementation that reaches the Engine to serve.
        second, listed, read = self._coverage(listing=self._unavailable,
                                              artifacts=self._unavailable)
        self.assertEqual(second, first)
        listed.assert_not_called()
        read.assert_not_called()

    def test_past_the_freshness_window_the_recheck_runs_behind_the_answer(self):
        first, *_ = self._coverage()
        self.clock.advance(engine.COVERAGE_FRESH_FOR + 1)
        second, listed, read = self._coverage()
        # The reader was answered from memory; the digest was rechecked after
        # the answer went out, and an unchanged digest refetches no artifact.
        self.assertEqual(second, first)
        listed.assert_called_once()
        read.assert_not_called()

    def test_a_recompiled_dataset_is_reflected_on_the_next_load(self):
        self._coverage(identity="digest-a")
        self.clock.advance(engine.COVERAGE_FRESH_FOR + 1)
        recompiled = [dict(ARTIFACTS[0], dataset_id="991")]
        _, _, read = self._coverage(identity="digest-b", artifacts=recompiled)
        # The digest moved, so the recheck fetched the artifacts again.
        read.assert_called_once()
        rows, listed, read = self._coverage(identity="digest-b")
        self.assertEqual([row["dataset_id"] for row in rows], ["991"])
        listed.assert_not_called()

    def test_a_dlm_this_process_generates_does_not_wait_for_a_recheck(self):
        self._coverage()
        with patch.object(engine, "_evict_answers", return_value=0):
            engine.invalidate_caches()
        # Immediately, not at the end of the staleness window.
        _, listed, read = self._coverage()
        listed.assert_called_once()
        read.assert_called_once()

    def test_one_readers_banner_is_not_served_to_another(self):
        self._coverage(actor="owner", role="Admin")
        # The payload is visibility filtered, so a Viewer who shares a catalog
        # state with an Admin is still shown only what they may see.
        _, listed, read = self._coverage(actor="guest", role="Viewer")
        listed.assert_called_once()
        read.assert_called_once()

    def test_an_answer_that_cannot_be_placed_in_the_catalog_is_not_remembered(self):
        # No digest means nothing says what state the answer reflects, so it is
        # handed to its caller and never served again. Slower, never stale.
        self._coverage(listing=self._unavailable)
        _, _, read = self._coverage(listing=self._unavailable)
        read.assert_called_once()
        self.assertEqual(self.cache.stats()["entries"], 0)

    def test_a_recheck_that_keeps_failing_stops_the_answer_being_served(self):
        self._coverage()
        # `validated_at` advances only on success, so continued failure ages
        # the entry past its bound rather than serving it for ever.
        self.clock.advance(engine.COVERAGE_MAX_SERVED_AGE + 1)
        with self.assertRaises(RuntimeError):
            self._coverage(listing=self._unavailable, artifacts=self._unavailable)

    def test_a_warm_cycle_leaves_the_first_reader_nothing_to_pay(self):
        # The point of defect D-27: somebody always paid the 2.1s cold read.
        # After a warm, the first reader pays nothing — no listing to establish
        # the digest and no artifact read behind the answer.
        with self._engine() as (listed, read):
            self.assertEqual(engine.warm_coverage("owner", "Admin"),
                             revalidating_cache.RevalidatingCache.WARM_VALIDATED)
            listed.assert_called_once()
            read.assert_called_once()
        rows, listed, read = self._coverage(actor="owner", role="Admin")
        listed.assert_not_called()
        read.assert_not_called()
        self.assertEqual([row["dataset_id"] for row in rows], ["136", "144"])

    def test_warming_again_inside_the_window_asks_the_engine_for_nothing(self):
        with self._engine() as (listed, read):
            engine.warm_coverage("owner", "Admin")
            listed.reset_mock()
            self.assertEqual(engine.warm_coverage("owner", "Admin"),
                             revalidating_cache.RevalidatingCache.WARM_FRESH)
            listed.assert_not_called()
            read.assert_called_once()

    def test_warming_is_for_one_principal_and_is_not_served_to_another(self):
        with self._engine() as (listed, read):
            engine.warm_coverage("owner", "Admin")
        # The payload is visibility filtered, so a Viewer nobody warmed for
        # reads for themselves rather than being handed the Admin's banner.
        _, listed, read = self._coverage(actor="guest", role="Viewer")
        listed.assert_called_once()
        read.assert_called_once()

    def test_warming_during_an_outage_raises_and_keeps_what_was_held(self):
        with self._engine() as (listed, read):
            engine.warm_coverage("owner", "Admin")
        self.clock.advance(engine.COVERAGE_FRESH_FOR + 1)
        with self._engine(listing=self._unavailable, artifacts=self._unavailable):
            with self.assertRaises(RuntimeError):
                engine.warm_coverage("owner", "Admin")
        # Reported to the warmer, and the entry it could not recheck is left
        # exactly as it stands, so the reader is still answered from memory —
        # the recheck the reader triggers runs behind their answer and, finding
        # the digest where it left it, refetches no artifact.
        rows, listed, read = self._coverage(actor="owner", role="Admin")
        self.assertEqual([row["dataset_id"] for row in rows], ["136", "144"])
        listed.assert_called_once()
        read.assert_not_called()

    def test_warming_without_a_principal_is_refused(self):
        with self.assertRaises(ValueError):
            engine.warm_coverage("")

    def test_one_recheck_runs_however_many_readers_arrive(self):
        self._coverage()
        self.clock.advance(engine.COVERAGE_FRESH_FOR + 1)

        in_flight = threading.Barrier(2, timeout=5)
        release = threading.Event()
        listings = []

        def _listing(actor):
            listings.append(actor)
            # The recheck is demonstrably running while readers are served:
            # a request-path implementation deadlocks here, which is half the
            # assertion. The other half is that no second recheck arrives.
            in_flight.wait()
            release.wait(5)
            return object()

        # Held open for the whole case, because the recheck is still inside it.
        with self._engine(listing=_listing) as (listed, read):
            first = engine.coverage("owner", "Admin")
            in_flight.wait()
            for _ in range(4):
                self.assertEqual(engine.coverage("owner", "Admin"), first)
            # Four more readers, no second recheck and no read of their own.
            listed.assert_called_once()
            read.assert_not_called()
            self.assertEqual(self.cache.stats()["in_flight"], 1)
            release.set()
            self.spawner.join()
        self.assertEqual(listings, ["owner"])


class CompiledArtifactsAreFetchedTogether(unittest.TestCase):
    """Nine compiled artifacts are nine independent object reads.

    Fetched one after another the page waited for each in turn, and the reads
    themselves are a fraction of that wall time. Measured in the cluster:
    0.045s to read an artifact, and nine of them cost the banner seconds.
    """

    def _serving(self, actor="owner", role="Admin"):
        state = engine._RetirementServingState(actor, role)
        return state, engine._RETIREMENT_SERVING.set(state)

    def test_every_artifact_is_read_and_returned_in_order(self):
        state, token = self._serving()
        try:
            with patch.object(engine, "get_dlm",
                              side_effect=lambda did, *a, **k: {"dataset_id": did}):
                read = engine._read_artifacts(["1", "2", "3"], context=None)
        finally:
            engine._RETIREMENT_SERVING.reset(token)
        self.assertEqual([entry["dataset_id"] for entry in read], ["1", "2", "3"])

    def test_reads_overlap_rather_than_waiting_in_turn(self):
        import threading
        state, token = self._serving()
        started = threading.Barrier(3, timeout=5)

        def _read(did, *args, **kwargs):
            # Every read must be in flight before any may finish: a sequential
            # implementation deadlocks here, which is the assertion.
            started.wait()
            return {"dataset_id": did}

        try:
            with patch.object(engine, "get_dlm", side_effect=_read):
                read = engine._read_artifacts(["1", "2", "3"], context=None)
        finally:
            engine._RETIREMENT_SERVING.reset(token)
        self.assertEqual(len(read), 3)

    def test_one_unreadable_artifact_is_reported_against_its_own_dataset(self):
        state, token = self._serving()
        boom = RuntimeError("artifact is corrupt")

        def _read(did, *args, **kwargs):
            if did == "2":
                raise boom
            return {"dataset_id": did}

        try:
            with patch.object(engine, "get_dlm", side_effect=_read):
                read = engine._read_artifacts(["1", "2", "3"], context=None)
        finally:
            engine._RETIREMENT_SERVING.reset(token)
        # The batch is not lost: the failure arrives in the slot it belongs to.
        self.assertEqual(read[0], {"dataset_id": "1"})
        self.assertIs(read[1], boom)
        self.assertEqual(read[2], {"dataset_id": "3"})

    def test_concurrency_is_bounded(self):
        state, token = self._serving()
        try:
            with patch.object(engine, "get_dlm", return_value=None),                  patch("dlm.engine.ThreadPoolExecutor") as pool:
                pool.return_value.__enter__.return_value.map.return_value = []
                engine._read_artifacts([str(n) for n in range(50)], context=None)
        finally:
            engine._RETIREMENT_SERVING.reset(token)
        # The request path must not open a connection per dataset.
        self.assertEqual(pool.call_args.kwargs["max_workers"],
                         engine._ARTIFACT_READ_CONCURRENCY)


if __name__ == "__main__":
    unittest.main()
