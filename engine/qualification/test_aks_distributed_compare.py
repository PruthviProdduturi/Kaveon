import unittest

from aks_distributed_compare import merge_stage_execution


class ExecutionSummaryTests(unittest.TestCase):
    def test_aggregates_additive_counters_and_stage_peaks(self):
        target = {}
        stages = [{"stage_id": 2, "tasks": [
            {"scan": {"object_metadata_cache_hits": 3},
             "execution": {"compute_cpu_us": 7, "exchange_input_bytes": 100,
                           "exchange_decode_bytes": 150, "memory_peak_bytes": 80,
                           "spill_peak_bytes": 20, "spill_bytes_written": 40,
                           "spill_runs_written": 2, "spill_compactions": 1,
                           "spill_compaction_input_bytes": 30,
                           "memory_reservation_calls": 5, "memory_reservation_bytes": 500,
                           "aggregate_input_rows": 80, "aggregate_groups_created": 8,
                           "aggregate_distinct_values_admitted": 4}},
            {"execution": {"compute_cpu_us": 11, "exchange_input_bytes": 200,
                           "exchange_decode_bytes": 250, "memory_peak_bytes": 60,
                           "spill_peak_bytes": 50, "spill_bytes_written": 70,
                           "spill_runs_written": 3, "spill_compactions": 2,
                           "spill_compaction_input_bytes": 90,
                           "memory_reservation_calls": 7, "memory_reservation_bytes": 700,
                           "aggregate_input_rows": 120, "aggregate_groups_created": 12,
                           "aggregate_distinct_values_admitted": 6}},
        ]}]
        merge_stage_execution(target, stages)
        merge_stage_execution(target, stages)
        summary = target["2"]
        self.assertEqual(summary["samples"], 2)
        self.assertEqual(summary["tasks_observed"], 4)
        self.assertEqual(summary["tasks_with_metrics"], 4)
        self.assertEqual(summary["tasks_with_cpu"], 4)
        self.assertEqual(summary["compute_cpu_us"], 36)
        self.assertEqual(summary["exchange_input_bytes"], 600)
        self.assertEqual(summary["spill_bytes_written"], 220)
        self.assertEqual(summary["spill_compactions"], 6)
        self.assertEqual(summary["memory_peak_bytes"], 80)
        self.assertEqual(summary["spill_peak_bytes"], 50)
        self.assertEqual(summary["object_metadata_cache_hits"], 6)
        self.assertEqual(summary["memory_reservation_calls"], 24)
        self.assertEqual(summary["memory_reservation_bytes"], 2400)
        self.assertEqual(summary["aggregate_input_rows"], 400)
        self.assertEqual(summary["aggregate_groups_created"], 40)
        self.assertEqual(summary["aggregate_distinct_values_admitted"], 20)

    def test_missing_metrics_remain_visible_as_coverage_gap(self):
        summary = merge_stage_execution({}, [{"stage_id": 4, "tasks": [{}, {"execution": None}]}])["4"]
        self.assertEqual(summary["tasks_observed"], 2)
        self.assertEqual(summary["tasks_with_metrics"], 0)
        self.assertEqual(summary["tasks_with_cpu"], 0)
        self.assertEqual(summary["object_metadata_cache_hits"], 0)


if __name__ == "__main__":
    unittest.main()
