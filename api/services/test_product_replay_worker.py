import os
import threading
import unittest
from unittest.mock import patch

from services import product_replay_worker as worker


class ProductReplayWorkerTests(unittest.TestCase):
    def setUp(self):
        with worker._lock:
            worker._state.update(
                enabled=False, running=False, last_success_at=None,
                last_error_at=None, last_error_type=None, last_batch_count=0,
                applied_total=0,
            )

    def test_default_off_does_not_create_thread(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertIsNone(worker.start())
        self.assertEqual(worker.status()["enabled"], False)

    def test_full_batch_is_drained_before_wait(self):
        stop = threading.Event()
        reports = [{"count": 2}, {"count": 1}]
        with patch.dict(os.environ, {
            "KAVEON_PRODUCT_REPLAY_BATCH_SIZE": "2",
            "KAVEON_PRODUCT_REPLAY_INTERVAL_SECONDS": "1",
        }), patch.object(worker.product_replay, "replay_pending", side_effect=reports) as replay, \
             patch.object(stop, "wait", side_effect=lambda _seconds: stop.set()):
            worker.run(stop)
        self.assertEqual(replay.call_count, 2)
        self.assertEqual(worker.status()["applied_total"], 3)
        self.assertFalse(worker.status()["running"])

    def test_failure_is_content_free_and_retried_after_wait(self):
        stop = threading.Event()
        with patch.dict(os.environ, {
            "KAVEON_PRODUCT_REPLAY_BATCH_SIZE": "1",
            "KAVEON_PRODUCT_REPLAY_INTERVAL_SECONDS": "1",
        }), patch.object(
            worker.product_replay, "replay_pending",
            side_effect=RuntimeError("secret source payload"),
        ), patch.object(stop, "wait", side_effect=lambda _seconds: stop.set()):
            worker.run(stop)
        report = worker.status()
        self.assertEqual(report["last_error_type"], "RuntimeError")
        self.assertNotIn("secret", str(report))
        self.assertIsNotNone(report["last_error_at"])

    def test_configuration_is_bounded(self):
        with patch.dict(os.environ, {"KAVEON_PRODUCT_REPLAY_BATCH_SIZE": "101"}):
            with self.assertRaisesRegex(RuntimeError, "between 1 and 100"):
                worker.run(threading.Event())


if __name__ == "__main__":
    unittest.main()
