"""`revalidating_cache` — serving from memory with the recheck behind it.

The reason this exists rather than an ordinary cache: establishing whether the
answer is still true costs about as much as the answer did, so a cache that
checks before answering never gets a reader below that cost. These cases pin
the properties that make answering first safe — a served answer makes no call
at all, one recheck runs however many readers arrive, the staleness has a
stated bound, a recheck that keeps failing stops the answer being served, and
a write is reflected at once rather than at the end of the window.

Nothing here asserts on elapsed time: the clock is moved by hand, and
concurrency is pinned with a barrier.
"""

import threading
import time
import unittest
from unittest.mock import patch

from services import revalidating_cache


class _Clock:
    def __init__(self):
        self.now = 1_000.0

    def advance(self, seconds):
        self.now += seconds

    def __call__(self):
        return self.now


class _Spawner:
    """Runs the recheck on a real thread, and lets a case wait for it."""

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


class _Validations:
    """A validation callback that records every call and answers as told."""

    def __init__(self, *answers):
        self.answers = list(answers)
        self.calls = []
        self.lock = threading.Lock()

    def __call__(self, previous):
        with self.lock:
            self.calls.append(previous)
            answer = (self.answers.pop(0) if len(self.answers) > 1
                      else self.answers[0])
        if isinstance(answer, BaseException):
            raise answer
        if answer == "unchanged":
            return previous
        return answer


class _Fixture(unittest.TestCase):
    def setUp(self):
        self.clock = _Clock()
        self.spawner = _Spawner()
        self.addCleanup(self.spawner.join)

    def _cache(self, *, fresh_for=10.0, max_served_age=100.0, **kwargs):
        return revalidating_cache.RevalidatingCache(
            "test", fresh_for=fresh_for, max_served_age=max_served_age,
            clock=self.clock, spawn=self.spawner, **kwargs)


class AnAnswerIsServedWithoutValidatingIt(_Fixture):
    def test_the_first_caller_validates_and_waits(self):
        cache, validate = self._cache(), _Validations(("first", "token-1"))
        self.assertEqual(cache.serve("k", validate), "first")
        self.assertEqual(validate.calls, [None])

    def test_a_caller_inside_the_freshness_window_calls_nothing(self):
        cache, validate = self._cache(), _Validations(("first", "token-1"))
        cache.serve("k", validate)
        self.clock.advance(9.0)
        boom = _Validations(RuntimeError("must not be consulted"))
        self.assertEqual(cache.serve("k", boom), "first")
        self.assertEqual(boom.calls, [])

    def test_past_the_window_the_answer_goes_out_before_the_recheck_finishes(self):
        cache = self._cache()
        validate = _Validations(("first", "token-1"))
        cache.serve("k", validate)
        self.clock.advance(11.0)

        entered = threading.Barrier(2, timeout=5)
        release = threading.Event()

        def _slow(previous):
            # The answer must already have been returned: a check-first
            # implementation blocks on this barrier instead, which is the
            # assertion.
            entered.wait()
            release.wait(5)
            return ("second", "token-2")

        self.assertEqual(cache.serve("k", _slow), "first")
        entered.wait()
        release.set()
        self.spawner.join()
        # And the rechecked answer is what the next reader gets.
        self.assertEqual(cache.serve("k", _Validations(("x", "x"))), "second")

    def test_an_unchanged_token_keeps_the_answer_that_was_already_held(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        validate = _Validations("unchanged")
        cache.serve("k", validate)
        self.spawner.join()
        # The callback was handed what is held, so it can say "nothing moved"
        # without rebuilding anything.
        self.assertEqual(validate.calls, [("first", "token-1")])
        self.assertEqual(cache.serve("k", _Validations(("x", "x"))), "first")


class OnlyOneRecheckRunsAtATime(_Fixture):
    def test_concurrent_readers_share_the_one_recheck(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)

        entered = threading.Barrier(2, timeout=5)
        release = threading.Event()
        validate = _Validations(("second", "token-2"))

        def _slow(previous):
            result = validate(previous)
            entered.wait()
            release.wait(5)
            return result

        self.assertEqual(cache.serve("k", _slow), "first")
        entered.wait()
        # Every reader that arrives while the recheck is in flight is served
        # the answer held, and none of them starts a second recheck.
        for _ in range(5):
            self.assertEqual(cache.serve("k", _slow), "first")
        self.assertEqual(cache.stats()["in_flight"], 1)
        release.set()
        self.spawner.join()
        self.assertEqual(len(validate.calls), 1)

    def test_a_cold_key_is_computed_once_for_everyone_waiting(self):
        cache = self._cache()
        together = threading.Barrier(4, timeout=5)
        release = threading.Event()
        validate = _Validations(("first", "token-1"))
        answers, failures = [], []
        lock = threading.Lock()

        def _slow(previous):
            result = validate(previous)
            release.wait(5)
            return result

        def _reader():
            try:
                together.wait()
                answer = cache.serve("k", _slow)
                with lock:
                    answers.append(answer)
            except BaseException as error:  # noqa: BLE001 - reported below
                with lock:
                    failures.append(error)

        readers = [threading.Thread(target=_reader) for _ in range(4)]
        for reader in readers:
            reader.start()
        # None may finish until all four are inside `serve`, so a per-caller
        # computation would show up as four validations rather than one.
        deadline = time.monotonic() + 10
        while cache.stats()["waited"] < 4 and time.monotonic() < deadline:
            time.sleep(0.005)
        self.assertEqual(cache.stats()["waited"], 4)
        release.set()
        for reader in readers:
            reader.join(timeout=10)
        self.assertEqual(failures, [])
        self.assertEqual(answers, ["first"] * 4)
        self.assertEqual(len(validate.calls), 1)


class StalenessIsBounded(_Fixture):
    def test_past_the_bound_the_caller_waits_for_a_fresh_answer(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(101.0)
        self.assertEqual(cache.serve("k", _Validations(("second", "token-2"))),
                         "second")

    def test_a_recheck_that_keeps_failing_eventually_reaches_the_caller(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        failing = _Validations(RuntimeError("the Engine is not answering"))

        # Inside the bound the reader is still served, because the answer held
        # is still within the window its freshness was asserted for.
        self.clock.advance(11.0)
        self.assertEqual(cache.serve("k", failing), "first")
        self.spawner.join()

        # `validated_at` never advanced, so the entry ages out and the failure
        # is reported rather than the answer being served for ever.
        self.clock.advance(100.0)
        with self.assertRaises(RuntimeError):
            cache.serve("k", failing)

    def test_a_failed_recheck_does_not_discard_the_answer_it_could_not_check(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        cache.serve("k", _Validations(RuntimeError("transient")))
        self.spawner.join()
        self.assertEqual(cache.serve("k", _Validations(("x", "x"))), "first")


class AnUnvalidatedAnswerIsNotRemembered(_Fixture):
    def test_a_none_token_answers_its_caller_and_is_not_stored(self):
        cache = self._cache()
        validate = _Validations(("built the slow way", None))
        self.assertEqual(cache.serve("k", validate), "built the slow way")
        self.assertEqual(cache.stats()["entries"], 0)
        # So the next caller does the work again rather than being served
        # something nothing placed in time.
        cache.serve("k", validate)
        self.assertEqual(len(validate.calls), 2)

    def test_it_also_forgets_an_answer_it_can_no_longer_place(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        cache.serve("k", _Validations(("unplaceable", None)))
        self.spawner.join()
        self.assertEqual(cache.stats()["entries"], 0)


class AWriteIsReflectedAtOnce(_Fixture):
    def test_invalidate_drops_what_is_held(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        cache.invalidate()
        # Not at the end of the staleness window: now.
        self.assertEqual(cache.serve("k", _Validations(("second", "token-2"))),
                         "second")

    def test_invalidate_can_name_one_key(self):
        cache = self._cache()
        cache.serve("a", _Validations(("a1", "token-a")))
        cache.serve("b", _Validations(("b1", "token-b")))
        cache.invalidate("a")
        self.assertEqual(cache.stats()["entries"], 1)
        self.assertEqual(cache.serve("b", _Validations(("x", "x"))), "b1")

    def test_a_recheck_in_flight_when_the_write_landed_does_not_store_its_result(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)

        entered = threading.Barrier(2, timeout=5)
        release = threading.Event()

        def _slow(previous):
            entered.wait()
            release.wait(5)
            return ("read before the write", "token-stale")

        cache.serve("k", _slow)
        entered.wait()
        # The write lands while the recheck is still reading.
        cache.invalidate()
        release.set()
        self.spawner.join()
        # What it read is already superseded, so it is not what the next
        # reader is given.
        after = _Validations(("read after the write", "token-fresh"))
        self.assertEqual(cache.serve("k", after), "read after the write")
        self.assertEqual(after.calls, [None])


class OneKeyPerReader(_Fixture):
    def test_each_key_is_answered_from_its_own_entry(self):
        cache = self._cache()
        self.assertEqual(cache.serve(("owner", "Admin"),
                                     _Validations(("admin view", "t"))),
                         "admin view")
        self.assertEqual(cache.serve(("guest", "Viewer"),
                                     _Validations(("viewer view", "t"))),
                         "viewer view")
        self.assertEqual(cache.serve(("owner", "Admin"),
                                     _Validations(("x", "x"))),
                         "admin view")

    def test_the_least_recently_validated_entry_goes_when_the_bound_is_reached(self):
        cache = self._cache(max_entries=2)
        for key in ("a", "b", "c"):
            cache.serve(key, _Validations((key, f"token-{key}")))
            self.clock.advance(1.0)
        self.assertEqual(cache.stats()["entries"], 2)
        # `a` was validated longest ago, so it is the one dropped.
        validate = _Validations(("a again", "token-a2"))
        self.assertEqual(cache.serve("a", validate), "a again")
        self.assertEqual(validate.calls, [None])


class WarmingPaysBeforeAReaderHasTo(_Fixture):
    """`warm` is the same validation run by something that is not a reader.

    So unlike `serve` it has no response to hide the work behind: it runs on
    the calling thread, says what it did, and raises what it hit — because the
    caller's whole job is to report what it could not fill.
    """

    def test_a_reader_after_a_warm_calls_nothing(self):
        cache = self._cache()
        validate = _Validations(("first", "token-1"))
        self.assertEqual(cache.warm("k", validate), cache.WARM_VALIDATED)
        boom = _Validations(RuntimeError("must not be consulted"))
        self.assertEqual(cache.serve("k", boom), "first")
        self.assertEqual(boom.calls, [])

    def test_warming_what_is_already_fresh_does_nothing(self):
        cache = self._cache()
        cache.warm("k", _Validations(("first", "token-1")))
        boom = _Validations(RuntimeError("must not be consulted"))
        self.assertEqual(cache.warm("k", boom), cache.WARM_FRESH)
        self.assertEqual(boom.calls, [])

    def test_past_the_window_it_rechecks_and_can_keep_what_is_held(self):
        cache = self._cache()
        cache.warm("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        validate = _Validations("unchanged")
        self.assertEqual(cache.warm("k", validate), cache.WARM_VALIDATED)
        self.assertEqual(validate.calls, [("first", "token-1")])

    def test_a_failure_reaches_the_warmer_and_leaves_the_entry_alone(self):
        cache = self._cache()
        cache.warm("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        with self.assertRaises(RuntimeError):
            cache.warm("k", _Validations(RuntimeError("the Engine is not answering")))
        # Nothing held was discarded, so the reader is still answered.
        self.assertEqual(cache.serve("k", _Validations(("x", "x"))), "first")
        self.spawner.join()

    def test_a_failed_warm_is_not_also_logged_with_a_traceback(self):
        cache = self._cache()
        with patch.object(revalidating_cache.logger, "warning") as logged:
            with self.assertRaises(RuntimeError):
                cache.warm("k", _Validations(RuntimeError("not answering")))
        # The warmer reports what it could not fill, once per cycle. A
        # traceback here as well would mean one per read per principal per
        # cycle for as long as the Engine is down, which is a steady state
        # here rather than an incident.
        logged.assert_not_called()

    def test_an_answer_it_cannot_place_is_reported_and_not_remembered(self):
        cache = self._cache()
        self.assertEqual(cache.warm("k", _Validations(("built the slow way", None))),
                         cache.WARM_UNVALIDATED)
        self.assertEqual(cache.stats()["entries"], 0)

    def test_a_warm_and_a_reader_arriving_together_cost_one_validation(self):
        cache = self._cache()
        entered = threading.Barrier(2, timeout=5)
        release = threading.Event()
        validate = _Validations(("first", "token-1"))

        def _slow(previous):
            result = validate(previous)
            entered.wait()
            release.wait(5)
            return result

        warmed, served = [], []
        warmer = threading.Thread(
            target=lambda: warmed.append(cache.warm("k", _slow)), daemon=True)
        warmer.start()
        entered.wait()
        # The reader waits on the warm already in flight rather than starting
        # a second one, which is what makes warming safe to run at any moment.
        reader = threading.Thread(
            target=lambda: served.append(cache.serve("k", _slow)), daemon=True)
        reader.start()
        deadline = time.monotonic() + 10
        while cache.stats()["waited"] < 1 and time.monotonic() < deadline:
            time.sleep(0.005)
        self.assertEqual(cache.stats()["waited"], 1)
        release.set()
        warmer.join(timeout=10)
        reader.join(timeout=10)
        self.assertEqual(warmed, [cache.WARM_VALIDATED])
        self.assertEqual(served, ["first"])
        self.assertEqual(len(validate.calls), 1)

    def test_a_warm_that_joins_a_reader_says_it_shared_the_recheck(self):
        cache = self._cache()
        cache.serve("k", _Validations(("first", "token-1")))
        self.clock.advance(11.0)
        entered = threading.Barrier(2, timeout=5)
        release = threading.Event()

        def _slow(previous):
            entered.wait()
            release.wait(5)
            return ("second", "token-2")

        cache.serve("k", _slow)      # the recheck is now behind the response
        entered.wait()
        shared = []
        warmer = threading.Thread(
            target=lambda: shared.append(cache.warm("k", _slow)), daemon=True)
        warmer.start()
        release.set()
        warmer.join(timeout=10)
        self.assertEqual(shared, [cache.WARM_SHARED])


class ItsBoundsMustBeStated(_Fixture):
    def test_a_freshness_window_wider_than_the_staleness_bound_is_refused(self):
        with self.assertRaises(ValueError):
            self._cache(fresh_for=100.0, max_served_age=10.0)

    def test_invalidate_all_reaches_every_registered_cache(self):
        first, second = self._cache(), self._cache()
        first.serve("k", _Validations(("a", "token-a")))
        second.serve("k", _Validations(("b", "token-b")))
        revalidating_cache.invalidate_all()
        self.assertEqual(first.stats()["entries"], 0)
        self.assertEqual(second.stats()["entries"], 0)


if __name__ == "__main__":
    unittest.main()
