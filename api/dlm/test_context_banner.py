"""`engine.coverage` — what the home page's available-context banner costs.

The banner names each compiled dataset, its date range, its row count and a few
indexed values. It used to pay, per artifact, a sample read, a count read, a
full dataset-document read and a COUNT(*) per table through the Engine, writing
the result back — and when a source could not answer that count nothing was
written, so the whole attempt repeated on every load of the page. These cases
pin it to reads: one for the artifacts, one for their values, and no write.
"""

import unittest
from unittest.mock import patch

from dlm import engine

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


if __name__ == "__main__":
    unittest.main()


class BannerIsServedFromWhatTheCatalogSaysAboutCompiledContent(unittest.TestCase):
    """The banner is recomputed when compiled content moves, and not otherwise.

    Rebuilding it means fetching every dataset's compiled artifact, and the
    home page paid that on every visit — including a reader who had just
    loaded it. The dataset, definition and run listings are what say whether
    anything compiled has changed, and they cost a fraction of the artifacts
    they point at, so the answer is kept against a digest of them.

    Deliberately not the catalog's own snapshot identity, which would have
    been cheaper to ask for and wrong: it advances on every product write, and
    the platform records its query history in that same catalog, so running
    one query would have invalidated a banner that could not have changed.
    """

    def setUp(self):
        engine._COVERAGE_CACHE.clear()
        self.addCleanup(engine._COVERAGE_CACHE.clear)

    def _coverage(self, identity, actor="owner", role="Admin"):
        """Render the banner as `actor`, with the catalog in a given state."""
        state = engine._RetirementServingState(actor, role)
        state.identity = identity
        state.context = object()  # already listed; nothing left to read for the key
        token = engine._RETIREMENT_SERVING.set(state)
        try:
            with patch.object(engine, "ensure_tables"),                  patch.object(engine, "_serving_artifacts",
                              return_value=ARTIFACTS) as artifacts,                  patch.object(engine, "_bulk_value_index",
                              return_value=({}, {})) as values:
                rows = engine.coverage()
            return rows, artifacts, values
        finally:
            engine._RETIREMENT_SERVING.reset(token)

    def test_unchanged_compiled_content_is_not_fetched_a_second_time(self):
        first, artifacts, values = self._coverage("digest-a")
        artifacts.assert_called_once()
        values.assert_called_once()

        second, artifacts, values = self._coverage("digest-a")
        # Nothing was fetched: the previous answer still describes this state.
        artifacts.assert_not_called()
        values.assert_not_called()
        self.assertEqual(second, first)

    def test_a_recompiled_dataset_is_reflected_on_the_next_load(self):
        self._coverage("digest-a")
        _, artifacts, _ = self._coverage("digest-b")
        artifacts.assert_called_once()

    def test_a_superseded_state_does_not_keep_its_entry(self):
        self._coverage("digest-a")
        self._coverage("digest-b")
        # One answer per state of the catalog, not one per visit for the life
        # of the process.
        self.assertEqual({key[0] for key in engine._COVERAGE_CACHE}, {"digest-b"})

    def test_one_readers_banner_is_not_served_to_another(self):
        self._coverage("digest-a", actor="owner", role="Admin")
        # Visibility is per caller, so a Viewer who shares a catalog state with
        # an Admin must still be shown only what they may see.
        _, artifacts, _ = self._coverage("digest-a", actor="guest", role="Viewer")
        artifacts.assert_called_once()

    def test_a_caller_that_cannot_list_is_answered_without_a_cache(self):
        # No key means recomputed every time. Slower, never stale.
        self._coverage(None)
        _, artifacts, _ = self._coverage(None)
        artifacts.assert_called_once()
        self.assertEqual(engine._COVERAGE_CACHE, {})


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
