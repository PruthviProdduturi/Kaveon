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

    def test_no_commits_yet_does_not_divide_by_zero(self):
        """A freshly started Engine has committed nothing."""
        self.assertEqual(
            qualify.check_commit_health(_healthy(attempts=0, committed=0)), [])


if __name__ == "__main__":
    unittest.main()
