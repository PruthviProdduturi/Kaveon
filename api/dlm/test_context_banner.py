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
