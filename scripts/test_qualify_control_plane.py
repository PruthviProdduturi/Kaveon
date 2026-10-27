"""The qualifier's judgement, tested without a cluster.

A qualification script is only worth running if it fails when it should. The
transport is exercised by pointing it at an Engine; the decisions are not --
those are reached from a metrics payload, and a check that cannot fail would
report a healthy control plane on a failing one.

So the checks are tested here against payloads that stand for the states they
exist to catch: an Engine predating the sharded format, a snapshot at its
ceiling, commits finishing indeterminate, and writers contending harder than
the local measurement.
"""
import importlib.util
import unittest
from pathlib import Path

# Loaded by path because the script is a command rather than a module, and
# renaming it to be importable would change how it is run.
_SPEC = importlib.util.spec_from_file_location(
    "qualify_control_plane", Path(__file__).with_name("qualify-control-plane.py")
)
qualify = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(qualify)


def _healthy(**overrides) -> dict:
    metrics = {
        "snapshot_bytes": 4_096,
        "snapshot_bytes_peak": 4_096,
        "snapshot_bytes_limit": qualify.MAX_SNAPSHOT_BYTES,
        "snapshot_percent_of_limit": 0,
        "attempts": 100,
        "committed": 100,
        "conflicts": 0,
        "rejected": 0,
        "indeterminate": 0,
    }
    metrics.update(overrides)
    return metrics


class ShardedCheck(unittest.TestCase):
    def test_an_engine_without_the_metric_is_refused(self):
        """The failure it exists for: an old Engine carrying the old ceiling.

        A deployment running the pre-eviction format looks identical to a
        current one until writes stop, so the absence of the metric has to be
        a failure rather than a missing field.
        """
        with self.assertRaises(qualify.Failed) as raised:
            qualify.check_snapshot_is_sharded({"attempts": 10, "committed": 10})
        self.assertIn("predates", str(raised.exception))

    def test_a_current_engine_passes(self):
        qualify.check_snapshot_is_sharded(_healthy())


class OccupancyCheck(unittest.TestCase):
    def test_a_snapshot_at_its_ceiling_fails(self):
        with self.assertRaises(qualify.Failed) as raised:
            qualify.check_occupancy(_healthy(snapshot_percent_of_limit=100))
        self.assertIn("rejected", str(raised.exception))

    def test_approaching_the_ceiling_warns_without_failing(self):
        warnings = qualify.check_occupancy(
            _healthy(snapshot_percent_of_limit=qualify.OCCUPANCY_WARN_PERCENT + 5))
        self.assertEqual(len(warnings), 1)
        self.assertIn("occupancy", warnings[0])

    def test_a_quiet_snapshot_says_nothing(self):
        self.assertEqual(qualify.check_occupancy(_healthy()), [])

    def test_occupancy_is_derived_when_the_engine_omits_it(self):
        """An older Engine reports the bytes without the percentage."""
        metrics = _healthy(snapshot_bytes_peak=qualify.MAX_SNAPSHOT_BYTES)
        del metrics["snapshot_percent_of_limit"]
        with self.assertRaises(qualify.Failed):
            qualify.check_occupancy(metrics)


class CommitHealthCheck(unittest.TestCase):
    def test_indeterminate_commits_are_reported(self):
        """Each is a write whose outcome is unknown, which is not a retry."""
        warnings = qualify.check_commit_health(_healthy(indeterminate=3))
        self.assertTrue(any("indeterminate" in warning for warning in warnings))

    def test_rejected_commits_are_reported(self):
        warnings = qualify.check_commit_health(_healthy(rejected=2))
        self.assertTrue(any("rejected" in warning for warning in warnings))

    def test_heavy_contention_is_reported(self):
        """Above the 2.12 attempts a commit measured at eight writers."""
        warnings = qualify.check_commit_health(_healthy(attempts=400, committed=100))
        self.assertTrue(any("attempts a commit" in warning for warning in warnings))

    def test_a_healthy_engine_says_nothing(self):
        self.assertEqual(qualify.check_commit_health(_healthy()), [])

    def test_an_engine_that_has_never_committed_says_so(self):
        """The vacuous pass this script was one line away from giving.

        Every other check passes on a freshly started Engine while nothing has
        been written, and the run read as a full qualification. A warning is
        the difference between qualifying the control plane and qualifying
        that it answers the phone.
        """
        warnings = qualify.check_commit_health(_healthy(attempts=0, committed=0))
        self.assertEqual(len(warnings), 1)
        self.assertIn("unexercised", warnings[0])

    def test_no_commits_yet_does_not_divide_by_zero(self):
        qualify.check_commit_health(_healthy(attempts=0, committed=0))


class WriteCheck(unittest.TestCase):
    """The only check that proves the commit path, and the only one that writes."""

    def setUp(self):
        self.calls = []
        self.base, self.token = "http://engine", "token"

    def _patch(self, sql_results, metrics_after):
        """Stand in for the Engine: record statements, answer in order."""
        import unittest.mock as mock

        def execute(sql, token, base, timeout=60.0):
            self.calls.append(sql)
            result = sql_results.pop(0)
            if isinstance(result, Exception):
                raise result
            return result

        return (
            mock.patch.object(qualify, "_execute_sql", side_effect=execute),
            mock.patch.object(qualify, "_request", return_value=metrics_after),
        )

    def test_a_write_that_reads_back_empty_is_a_failure(self):
        """The dangerous case: the commit reports success and nothing landed."""
        execute, request = self._patch(
            [{}, {}, {"rows": []}, {}], {"committed": 10})
        with execute, request, self.assertRaises(qualify.Failed) as raised:
            qualify.check_writes(self.base, self.token, {"committed": 7})
        self.assertIn("came back empty", str(raised.exception))

    def test_the_table_is_dropped_even_when_the_read_fails(self):
        """A qualification must not leave debris in a real catalogue."""
        execute, request = self._patch(
            [{}, {}, qualify.Failed("select exploded"), {}], {"committed": 10})
        with execute, request, self.assertRaises(qualify.Failed):
            qualify.check_writes(self.base, self.token, {"committed": 7})
        self.assertTrue(
            any(sql.startswith("DROP TABLE") for sql in self.calls),
            f"the table must be dropped on the way out, got: {self.calls}",
        )

    def test_a_commit_counter_that_does_not_move_is_a_failure(self):
        """Statements that appear to work while nothing commits."""
        execute, request = self._patch(
            [{}, {}, {"rows": [{"id": 1}]}, {}], {"committed": 7})
        with execute, request, self.assertRaises(qualify.Failed) as raised:
            qualify.check_writes(self.base, self.token, {"committed": 7})
        self.assertIn("commit counter", str(raised.exception))

    def test_a_healthy_write_path_passes_and_cleans_up(self):
        execute, request = self._patch(
            [{}, {}, {"rows": [{"id": 1, "note": "qualification"}]}, {}],
            {"committed": 11})
        with execute, request:
            warnings = qualify.check_writes(self.base, self.token, {"committed": 7})
        self.assertEqual(warnings, [])
        self.assertEqual(len(self.calls), 4)
        self.assertTrue(self.calls[0].startswith("CREATE TABLE"))
        self.assertTrue(self.calls[-1].startswith("DROP TABLE"))
        # Named per run, so two qualifications cannot collide on one table.
        self.assertIn("kaveon_qualify_", self.calls[0])


if __name__ == "__main__":
    unittest.main()
