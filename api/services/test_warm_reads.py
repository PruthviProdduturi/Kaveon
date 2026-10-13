"""`warm_reads` — paying for the hot reads before a reader has to.

What these cases are for: the first visit after a restart used to pay the whole
cost of every read the page makes, and so did the first visit after anything
invalidated (defect D-27). The claim being pinned is that a cycle has already
paid it — a read taken after one makes no Engine call at all — and that the
cycle is safe to run in the conditions this installation is actually in: the
Engine scaled to zero by a benchmark campaign for long stretches, several
replicas, and a cycle that can outrun its own interval.

Nothing here asserts on elapsed time. The clock is moved by hand, concurrency
is pinned with a barrier, and the loop's waiting is a callable a case supplies.
"""

import threading
import time
import unittest
from unittest.mock import Mock, patch

from fastapi import HTTPException

from services import product_read_authority, revalidating_cache, warm_reads

WARM = revalidating_cache.RevalidatingCache

CHARTS = [{"id": "c-1", "generation": 3, "updated_at": "2026-10-01T00:00:00Z"},
          {"id": "c-2", "generation": 1, "updated_at": "2026-09-01T00:00:00Z"}]


class _Clock:
    def __init__(self):
        self.now = 1_000.0

    def advance(self, seconds):
        self.now += seconds

    def __call__(self):
        return self.now


class _Spawner:
    """Runs a recheck behind a response on a real thread, and lets a case wait."""

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


class _Fixture(unittest.TestCase):
    """A warmer with no principals but the ones a case states, and its own cache."""

    def setUp(self):
        self.clock = _Clock()
        self.spawner = _Spawner()
        self.addCleanup(self.spawner.join)
        warm_reads.forget_principals()
        self.addCleanup(warm_reads.forget_principals)

        self.cache = revalidating_cache.RevalidatingCache(
            "test-product-listings", fresh_for=warm_reads.LISTING_FRESH_FOR,
            max_served_age=warm_reads.LISTING_MAX_SERVED_AGE,
            clock=self.clock, spawn=self.spawner)
        self._patch(patch.object(warm_reads, "_LISTINGS", self.cache))
        self._patch(patch.object(product_read_authority, "enabled", return_value=True))
        # The context banner has its own cache and its own cases
        # (dlm/test_context_banner.py); here it stands in as one more task so a
        # cycle's shape is what is under test.
        self.coverage = Mock(return_value=WARM.WARM_VALIDATED)
        self._patch(patch.object(warm_reads, "_warm_coverage", self.coverage))
        self._settings(KAVEON_READ_WARMER_ENABLED=True, KAVEON_WARM_PRINCIPALS="",
                       KAVEON_WARM_START_DELAY_SECONDS=5.0,
                       KAVEON_WARM_INTERVAL_SECONDS=60.0)

    def _patch(self, patcher):
        patcher.start()
        self.addCleanup(patcher.stop)

    def _settings(self, **values):
        for name, value in values.items():
            self._patch(patch.object(warm_reads.settings, name, value))

    def _listings(self, documents=None, error=None):
        """Stand in for the one Engine round trip a listing costs."""
        if error is not None:
            read = Mock(side_effect=error)
        else:
            read = Mock(side_effect=lambda family, actor, role:
                        list(CHARTS if documents is None else documents))
        return patch.object(product_read_authority, "list_documents", read), read


class AWarmCycleFillsTheCacheBeforeAnybodyAsks(_Fixture):
    def test_a_listing_read_after_a_cycle_makes_no_engine_call(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher:
            cycle = warm_reads.run_cycle()
            self.assertEqual(cycle.principals, 1)
            # One listing per warmed family, and nothing else.
            self.assertEqual(sorted(call.args[0] for call in read.call_args_list),
                             sorted(warm_reads.LISTING_FAMILIES))
            read.reset_mock()

            # The read the Library would make: zero round trips.
            documents = warm_reads.listing("charts", "owner@example.test", "Admin")
            read.assert_not_called()
        self.assertEqual([item["id"] for item in documents], ["c-1", "c-2"])
        self.assertEqual(cycle.filled, len(warm_reads.LISTING_FAMILIES) + 1)
        self.assertEqual(cycle.unavailable, 0)

    def test_the_banner_is_warmed_for_the_principal_that_will_read_it(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, _ = self._listings()
        with patcher:
            warm_reads.run_cycle()
        self.coverage.assert_called_once_with("owner@example.test", "Admin")

    def test_a_second_cycle_inside_the_window_asks_the_engine_for_nothing(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher:
            warm_reads.run_cycle()
            read.reset_mock()
            self.clock.advance(warm_reads.LISTING_FRESH_FOR - 1)
            cycle = warm_reads.run_cycle()
        read.assert_not_called()
        self.assertEqual(cycle.fresh, len(warm_reads.LISTING_FAMILIES))

    def test_a_cycle_past_the_window_rechecks_and_keeps_what_has_not_moved(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher:
            warm_reads.run_cycle()
            self.clock.advance(warm_reads.LISTING_FRESH_FOR + 1)
            read.reset_mock()
            cycle = warm_reads.run_cycle()
            self.assertEqual(len(read.call_args_list), len(warm_reads.LISTING_FAMILIES))
            read.reset_mock()
            # And the reader it was warmed for still pays nothing.
            warm_reads.listing("charts", "owner@example.test", "Admin")
            read.assert_not_called()
        self.assertEqual(cycle.filled, len(warm_reads.LISTING_FAMILIES) + 1)

    def test_a_recheck_sees_a_favorite_move_without_the_record_moving(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        held = [dict(CHARTS[0], favorite=False)]
        patcher, _ = self._listings(documents=held)
        with patcher:
            warm_reads.run_cycle()
            # A favorite is the one thing on a Library card that moves without
            # the record it decorates moving, so it has to be in the token.
            held[0] = dict(held[0], favorite=True)
            self.clock.advance(warm_reads.LISTING_FRESH_FOR + 1)
            warm_reads.run_cycle()
            documents = warm_reads.listing("charts", "owner@example.test", "Admin")
        self.assertEqual(documents[0]["favorite"], True)

    def test_a_family_that_has_not_moved_to_kaveondb_is_not_read_at_all(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher, patch.object(product_read_authority, "enabled",
                                   return_value=False):
            cycle = warm_reads.run_cycle()
        read.assert_not_called()
        self.assertEqual(cycle.not_applicable, len(warm_reads.LISTING_FAMILIES))
        self.assertEqual(cycle.unavailable, 0)

    def test_nothing_is_warmed_when_nobody_is_configured_or_has_been_seen(self):
        patcher, read = self._listings()
        with patcher:
            cycle = warm_reads.run_cycle()
        read.assert_not_called()
        self.coverage.assert_not_called()
        self.assertEqual(cycle.principals, 0)


class AnEngineOutageFailsSafe(_Fixture):
    """The Engine is at zero replicas for long stretches, by design.

    A cycle then has nothing to warm with. What it must not do is discard what
    it already holds, report success, or take the process down with it.
    """

    def setUp(self):
        super().setUp()
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        self.outage = HTTPException(503, "KaveonDB is unavailable")

    def test_a_cycle_during_an_outage_leaves_the_previous_entry_intact(self):
        patcher, read = self._listings()
        with patcher:
            warm_reads.run_cycle()
        self.clock.advance(warm_reads.LISTING_FRESH_FOR + 1)

        patcher, failing = self._listings(error=self.outage)
        with patcher:
            cycle = warm_reads.run_cycle()
            self.assertEqual(cycle.unavailable, len(warm_reads.LISTING_FAMILIES))
            self.assertEqual(cycle.filled, 1)  # the banner's own stub
            # What was already held is exactly as it was, and is still served.
            self.assertEqual(self.cache.stats()["entries"],
                             len(warm_reads.LISTING_FAMILIES))
            documents = warm_reads.listing("charts", "owner@example.test", "Admin")
            self.assertEqual([item["id"] for item in documents], ["c-1", "c-2"])
            self.spawner.join()
        self.assertIn("could not warm", cycle.line())
        self.assertIn("503", cycle.line())
        self.assertGreater(failing.call_count, 0)

    def test_a_cycle_with_nothing_warm_to_fall_back_on_does_not_raise(self):
        patcher, _ = self._listings(error=self.outage)
        with patcher, patch.object(warm_reads, "_warm_coverage",
                                   side_effect=self.outage):
            cycle = warm_reads.run_cycle()
        self.assertEqual(cycle.unavailable, len(warm_reads.LISTING_FAMILIES) + 1)
        self.assertEqual(cycle.filled, 0)
        self.assertEqual(self.cache.stats()["entries"], 0)

    def test_the_loop_survives_a_cycle_that_fails_outright(self):
        stop = threading.Event()
        cycles = Mock(side_effect=[RuntimeError("unreadable principal ledger"),
                                  warm_reads.Cycle()])
        waits = []

        def _wait(seconds):
            waits.append(seconds)
            # Stop after the second interval, so the loop had to survive the
            # first cycle raising in order to run the second.
            return len(waits) > 2

        with patch.object(warm_reads, "run_cycle", cycles), \
             patch.object(warm_reads.logger, "exception") as reported:
            warm_reads._loop(stop, delay=5.0, interval=60.0, sleep=_wait)
        self.assertEqual(cycles.call_count, 2)
        reported.assert_called_once()
        self.assertEqual(waits, [5.0, 60.0, 60.0])


class ACycleAccountsForItselfInOneLine(_Fixture):
    def test_a_healthy_cycle_logs_one_summary_and_nothing_else(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, _ = self._listings()
        waits = []
        with patcher, \
             patch.object(warm_reads.logger, "info") as summary, \
             patch.object(warm_reads.logger, "debug") as debug, \
             patch.object(warm_reads.logger, "exception") as reported:
            warm_reads._loop(threading.Event(), delay=5.0, interval=60.0,
                             sleep=lambda seconds: bool(waits.append(seconds)
                                                        or len(waits) > 1))
        # One line per cycle in the steady state, no debug logging, and
        # `exception` kept for a real failure.
        summary.assert_called_once()
        debug.assert_not_called()
        reported.assert_not_called()
        self.assertIn("warm cycle: principals=1 filled=5",
                      summary.call_args.args[1])

    def test_the_line_names_what_it_could_not_warm_and_why(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, _ = self._listings(error=HTTPException(504, "coordinator timed out"))
        with patcher:
            line = warm_reads.run_cycle().line()
        self.assertIn("unreachable=4", line)
        self.assertIn("charts/owner@example.test: HTTPException 504", line)


class OnlyOneCycleRunsAtATime(_Fixture):
    def test_a_cycle_arriving_while_one_is_running_is_told_it_was_skipped(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        inside = threading.Barrier(2, timeout=10)
        release = threading.Event()
        reads = []

        def _slow(family, actor, role):
            reads.append(family)
            if len(reads) == 1:
                inside.wait()
                release.wait(10)
            return list(CHARTS)

        with patch.object(product_read_authority, "list_documents", _slow):
            first = threading.Thread(target=warm_reads.run_cycle, daemon=True)
            first.start()
            inside.wait()
            # A second cycle — the interval's next tick, or an operator — must
            # not double the Engine's load while this one is still reading.
            second = warm_reads.run_cycle()
            self.assertEqual(second.skipped, 1)
            self.assertEqual(second.results, [])
            self.assertEqual(second.line(),
                             "warm cycle skipped: one was already running")
            taken = len(reads)
            release.set()
            first.join(timeout=10)
            self.assertFalse(first.is_alive())
        # The skipped cycle read nothing of its own.
        self.assertLessEqual(taken, len(warm_reads.LISTING_FAMILIES))

    def test_a_reader_arriving_mid_warm_shares_the_one_read(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        inside = threading.Barrier(2, timeout=10)
        release = threading.Event()
        calls = []

        def _slow(family, actor, role):
            calls.append(family)
            if family == "charts":
                inside.wait()
                release.wait(10)
            return list(CHARTS)

        with patch.object(product_read_authority, "list_documents", _slow):
            cycle = threading.Thread(target=warm_reads.run_cycle, daemon=True)
            cycle.start()
            inside.wait()
            reader = []
            waiting = threading.Thread(
                target=lambda: reader.append(
                    warm_reads.listing("charts", "owner@example.test", "Admin")),
                daemon=True)
            waiting.start()
            # The reader waits on the warm already in flight rather than
            # starting a second listing of the same family — so it may not be
            # released until it is demonstrably inside `serve`.
            deadline = time.monotonic() + 10
            while not self.cache.stats()["waited"] and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertEqual(self.cache.stats()["waited"], 1)
            release.set()
            cycle.join(timeout=10)
            waiting.join(timeout=10)
        self.assertEqual(calls.count("charts"), 1)
        self.assertEqual([item["id"] for item in reader[0]], ["c-1", "c-2"])


class TheFlagDecidesWhetherAnythingStarts(_Fixture):
    def _threads(self):
        return [thread.name for thread in threading.enumerate()
                if thread.name == "read-warmer"]

    def test_disabling_it_starts_nothing(self):
        self._settings(KAVEON_READ_WARMER_ENABLED=False)
        self.assertIsNone(warm_reads.start())
        self.assertEqual(self._threads(), [])
        self.assertFalse(warm_reads.status()["running"])
        # Stopping what was never started is not an error either.
        warm_reads.stop(None)

    def test_starting_twice_does_not_start_a_second_loop(self):
        handle = warm_reads.start()
        self.addCleanup(warm_reads.stop, handle)
        self.assertIsNotNone(handle)
        self.assertIs(warm_reads.start(), handle)
        self.assertEqual(len(self._threads()), 1)

    def test_stopping_it_ends_the_thread_rather_than_leaving_it_behind(self):
        handle = warm_reads.start()
        warm_reads.stop(handle)
        self.assertFalse(handle.thread.is_alive())
        self.assertEqual(self._threads(), [])

    def test_the_first_cycle_waits_so_startup_is_never_blocked(self):
        cycles = Mock(return_value=warm_reads.Cycle())
        with patch.object(warm_reads, "run_cycle", cycles):
            # Stop during the startup delay: a warmer that ran its first cycle
            # at start time would have warmed something by now.
            warm_reads._loop(threading.Event(), delay=5.0, interval=60.0,
                             sleep=lambda seconds: seconds == 5.0)
        cycles.assert_not_called()

    def test_the_interval_cannot_be_set_past_what_it_is_keeping_fresh(self):
        self._settings(KAVEON_WARM_INTERVAL_SECONDS=3600.0)
        # An interval past the staleness bound would let an entry age out
        # between cycles, which is the cost this exists to remove.
        self.assertLessEqual(warm_reads._interval(),
                             warm_reads.LISTING_MAX_SERVED_AGE / 2.0)
        self._settings(KAVEON_WARM_INTERVAL_SECONDS=0.0)
        self.assertEqual(warm_reads._interval(), 1.0)


class WarmingIsPerPrincipalOrItIsWrong(_Fixture):
    def test_one_principals_warm_listing_is_never_served_to_another(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        internal = [{"id": "c-9", "generation": 1, "updated_at": "2026-10-02T00:00:00Z"}]
        read = Mock(side_effect=lambda family, actor, role:
                    list(CHARTS if role == "Admin" else internal))
        with patch.object(product_read_authority, "list_documents", read):
            warm_reads.run_cycle()
            read.reset_mock()
            # A Viewer nobody warmed for is answered from their own read, not
            # from the Admin's entry.
            documents = warm_reads.listing("charts", "guest@example.test", "Viewer")
        self.assertEqual([item["id"] for item in documents], ["c-9"])
        self.assertEqual(read.call_args.args, ("charts", "guest@example.test", "Viewer"))

    def test_a_reader_cannot_change_what_the_next_reader_is_served(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher:
            warm_reads.run_cycle()
            first = warm_reads.listing("charts", "owner@example.test", "Admin")
            first.pop()
            first[0]["name"] = "renamed in place"
            second = warm_reads.listing("charts", "owner@example.test", "Admin")
        self.assertEqual([item["id"] for item in second], ["c-1", "c-2"])
        self.assertNotIn("name", second[0])

    def test_a_write_is_reflected_at_once_rather_than_at_the_end_of_the_window(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        patcher, read = self._listings()
        with patcher:
            warm_reads.run_cycle()
            warm_reads.invalidate()
            self.assertEqual(self.cache.stats()["entries"], 0)
            read.reset_mock()
            warm_reads.listing("charts", "owner@example.test", "Admin")
        read.assert_called_once()

    def test_an_unknown_family_is_refused_rather_than_read(self):
        with self.assertRaises(ValueError):
            warm_reads.listing("query_history", "owner@example.test", "Admin")


class WhoIsWarmedFor(_Fixture):
    def test_a_configured_principal_is_warmed_before_anybody_has_been_seen(self):
        self._settings(KAVEON_WARM_PRINCIPALS=
                       "owner@example.test:Admin, analyst@example.test:Analyst")
        self.assertEqual(warm_reads.principals(),
                         [("owner@example.test", "Admin"),
                          ("analyst@example.test", "Analyst")])

    def test_a_configured_entry_naming_no_role_is_read_as_the_narrowest(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test")
        self.assertEqual(warm_reads.principals(), [("owner@example.test", "Viewer")])

    def test_an_entry_naming_an_unknown_role_is_rejected_not_widened(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Root,  ,b@x.test:Admin")
        self.assertEqual(warm_reads.principals(), [("b@x.test", "Admin")])

    def test_a_principal_who_has_read_is_warmed_without_being_configured(self):
        warm_reads.note_principal("seen@example.test", "Editor", now=100.0)
        self.assertEqual(warm_reads.principals(now=100.0),
                         [("seen@example.test", "Editor")])

    def test_a_principal_who_has_gone_quiet_stops_being_warmed_for(self):
        warm_reads.note_principal("seen@example.test", "Editor", now=100.0)
        self.assertEqual(warm_reads.principals(now=100.0 + warm_reads.PRINCIPAL_TTL), [
            ("seen@example.test", "Editor")])
        self.assertEqual(
            warm_reads.principals(now=100.0 + warm_reads.PRINCIPAL_TTL + 1), [])

    def test_the_ledger_is_bounded_and_drops_the_longest_quiet_first(self):
        for index in range(warm_reads.MAX_PRINCIPALS + 3):
            warm_reads.note_principal(f"p{index}@example.test", "Viewer",
                                      now=100.0 + index)
        seen = warm_reads.principals(now=200.0)
        self.assertEqual(len(seen), warm_reads.MAX_PRINCIPALS)
        self.assertNotIn(("p0@example.test", "Viewer"), seen)
        self.assertIn(("p10@example.test", "Viewer"), seen)

    def test_an_unusable_identity_is_not_recorded(self):
        warm_reads.note_principal("", "Admin")
        warm_reads.note_principal("someone@example.test", "Superuser")
        self.assertEqual(warm_reads.principals(), [])

    def test_a_configured_principal_is_not_warmed_twice_for_being_seen_too(self):
        self._settings(KAVEON_WARM_PRINCIPALS="owner@example.test:Admin")
        warm_reads.note_principal("owner@example.test", "Admin", now=100.0)
        self.assertEqual(warm_reads.principals(now=100.0),
                         [("owner@example.test", "Admin")])


if __name__ == "__main__":
    unittest.main()
