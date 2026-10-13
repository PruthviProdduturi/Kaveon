"""The floor under the Engine's replica count.

A benchmark campaign scales the Engine to zero to give the other query engine
the nodes, and a campaign that fails or is interrupted leaves it there. These
cases pin the two properties that matter: the floor is restored, and a
deliberate scale above it is left alone.
"""

import unittest
from unittest.mock import patch

from services import availability_guard as guard


class FakeCluster:
    """Records scale reads and writes, and can be told to fail either."""

    def __init__(self, replicas, fail_read=(), fail_scale=()):
        self.replicas = dict(replicas)
        self.fail_read = set(fail_read)
        self.fail_scale = set(fail_scale)
        self.writes = []

    def scale(self, name):
        if name in self.fail_read:
            raise RuntimeError("the API refused the read")
        return self.replicas.get(name)

    def set_scale(self, name, replicas):
        if name in self.fail_scale:
            raise RuntimeError("the API refused the write")
        self.replicas[name] = replicas
        self.writes.append((name, replicas))
        return replicas


class TheFloorIsRestored(unittest.TestCase):
    def test_an_engine_scaled_to_nothing_is_brought_back(self):
        cluster = FakeCluster({"kaveon-coordinator": 0, "kaveon-worker": 0})
        result = guard.enforce(cluster)
        self.assertEqual(sorted(cluster.writes),
                         [("kaveon-coordinator", 1), ("kaveon-worker", 1)])
        self.assertEqual({entry["statefulset"] for entry in result["raised"]},
                         {"kaveon-coordinator", "kaveon-worker"})
        self.assertEqual(result["failed"], [])

    def test_only_what_is_below_its_floor_is_touched(self):
        cluster = FakeCluster({"kaveon-coordinator": 1, "kaveon-worker": 0})
        guard.enforce(cluster)
        self.assertEqual(cluster.writes, [("kaveon-worker", 1)])


class ADeliberateScaleIsLeftAlone(unittest.TestCase):
    def test_a_benchmark_measuring_with_three_workers_keeps_three(self):
        cluster = FakeCluster({"kaveon-coordinator": 1, "kaveon-worker": 3})
        result = guard.enforce(cluster)
        # The guard scales up to the floor, never down to it.
        self.assertEqual(cluster.writes, [])
        self.assertEqual({entry["replicas"] for entry in result["held"]}, {1, 3})

    def test_the_resting_set_is_not_reduced(self):
        cluster = FakeCluster({"kaveon-coordinator": 1, "kaveon-worker": 4})
        guard.enforce(cluster)
        self.assertEqual(cluster.writes, [])
        self.assertEqual(cluster.replicas["kaveon-worker"], 4)


class OneFailureDoesNotStopTheOther(unittest.TestCase):
    def test_an_unreadable_statefulset_does_not_block_the_rest(self):
        cluster = FakeCluster({"kaveon-coordinator": 0, "kaveon-worker": 0},
                              fail_read={"kaveon-coordinator"})
        result = guard.enforce(cluster)
        # The platform is more available with workers and no coordinator than
        # with neither, so the second is still checked.
        self.assertEqual(cluster.writes, [("kaveon-worker", 1)])
        self.assertEqual([entry["statefulset"] for entry in result["failed"]],
                         ["kaveon-coordinator"])

    def test_a_refused_scale_is_reported_not_raised(self):
        cluster = FakeCluster({"kaveon-coordinator": 0, "kaveon-worker": 0},
                              fail_scale={"kaveon-worker"})
        result = guard.enforce(cluster)
        self.assertEqual(cluster.writes, [("kaveon-coordinator", 1)])
        self.assertEqual([entry["stage"] for entry in result["failed"]], ["scale"])

    def test_a_missing_replica_count_is_a_failure_not_a_zero(self):
        # Treating an absent count as zero would scale a statefulset whose
        # state is unknown, which is a write made on no information.
        cluster = FakeCluster({"kaveon-coordinator": None, "kaveon-worker": 2})
        result = guard.enforce(cluster)
        self.assertEqual(cluster.writes, [])
        self.assertEqual([entry["statefulset"] for entry in result["failed"]],
                         ["kaveon-coordinator"])


class TheWorkerFloorIsConfigurable(unittest.TestCase):
    def test_the_deployment_may_raise_the_worker_floor(self):
        with patch.dict("os.environ", {"KAVEON_GUARD_MIN_WORKERS": "2"}):
            self.assertEqual(dict(guard.floors())["kaveon-worker"], 2)

    def test_a_floor_below_one_is_refused(self):
        # A floor of zero is the outage this exists to prevent, so it cannot be
        # configured back in.
        for value in ("0", "-3", "", "nonsense"):
            with patch.dict("os.environ", {"KAVEON_GUARD_MIN_WORKERS": value}):
                self.assertEqual(dict(guard.floors())["kaveon-worker"], 1, value)

    def test_the_coordinator_floor_is_not_configurable(self):
        with patch.dict("os.environ", {"KAVEON_GUARD_MIN_WORKERS": "9"}):
            self.assertEqual(dict(guard.floors())["kaveon-coordinator"], 1)


class TheExitStatusSaysWhatHappened(unittest.TestCase):
    def test_holding_the_floor_is_a_success(self):
        cluster = FakeCluster({"kaveon-coordinator": 1, "kaveon-worker": 3})
        with patch.object(guard, "Kubernetes", return_value=cluster):
            self.assertEqual(guard.main(), 0)

    def test_a_failure_is_reported_as_one(self):
        cluster = FakeCluster({"kaveon-coordinator": 0, "kaveon-worker": 0},
                              fail_scale={"kaveon-worker"})
        with patch.object(guard, "Kubernetes", return_value=cluster):
            self.assertEqual(guard.main(), 1)

    def test_being_unable_to_reach_the_api_is_distinct(self):
        with patch.object(guard, "Kubernetes", side_effect=RuntimeError("no host")):
            self.assertEqual(guard.main(), 2)


if __name__ == "__main__":
    unittest.main()
