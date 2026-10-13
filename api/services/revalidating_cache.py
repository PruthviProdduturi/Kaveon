"""An answer served from memory while its truth is rechecked behind the reader.

The ordinary cache asks "has anything changed?" before it answers. That works
when asking is cheap. It is not here: establishing whether a compiled DLM has
moved means listing the dataset, definition and run families, and the Engine
answers any product listing in about half a second regardless of how few
records it holds (defect D-3). So a cache that checks first pays ~1.4s on a
*hit* — the home page's context banner measured 1.45s warm against 2.82s cold,
and the hit was only saving the artifact reads.

This inverts the order. The reader is answered from memory with no round trip
at all, and the recheck runs behind the response. Three properties make that
safe to do:

**Bounded staleness.** An entry carries the moment its truth was last
confirmed. Inside ``fresh_for`` it is served and nothing is started — which
collapses the several reads one page load makes into one. Past that it is
still served immediately, but a recheck is started behind the response, so a
reader sees a superseded answer at most once. Past ``max_served_age`` it is not
served at all: the request waits for the recheck. ``validated_at`` advances
only on success, so a recheck that keeps failing walks the entry past that
bound and the failure reaches the caller instead of being papered over
indefinitely.

**One recheck at a time.** A recheck in flight is shared: concurrent callers
either take the stale answer or wait on the one already running, and never
start a second.

**No memory of an unvalidated answer.** The callback returns the answer and the
token it was computed from. A ``None`` token means nothing established what
state the answer reflects — it is handed to its caller and deliberately not
stored, because an answer we could not place in time must not be served later
as though we could.

Writes this process performs do not wait for a recheck: ``invalidate`` drops
the entry, and an in-flight recheck that started before the write is prevented
from storing its result (it carries the epoch it began in). A write performed
by *another* process is the case ``max_served_age`` exists to bound.
"""

from __future__ import annotations

import logging
import threading
import time
import weakref
from typing import Any, Callable, Dict, Hashable, Optional, Tuple

logger = logging.getLogger(__name__)

# What a validation callback returns: the answer, and a token identifying the
# state it was computed from. The token is compared, never interpreted.
Validated = Tuple[Any, Optional[str]]
Validate = Callable[[Optional[Validated]], Validated]


class _Entry:
    __slots__ = ("value", "token", "validated_at")

    def __init__(self, value: Any, token: str, validated_at: float):
        self.value = value
        self.token = token
        self.validated_at = validated_at


class _Flight:
    """One validation, and whatever it produced, for everyone waiting on it."""

    __slots__ = ("done", "value", "token", "error", "epoch")

    def __init__(self, epoch: int):
        self.done = threading.Event()
        self.value: Any = None
        self.token: Optional[str] = None
        self.error: Optional[BaseException] = None
        self.epoch = epoch

    def finish(self, value: Any, token: Optional[str]) -> None:
        self.value, self.token = value, token
        self.done.set()

    def fail(self, error: BaseException) -> None:
        self.error = error
        self.done.set()

    def result(self, timeout: float) -> Any:
        if not self.done.wait(timeout):
            raise TimeoutError("Revalidation did not finish within its bound")
        if self.error is not None:
            raise self.error
        return self.value


# Registered weakly: a cache is kept alive by whoever holds it, not by being
# registered, so this cannot turn into a list of every cache ever built.
_REGISTRY: "weakref.WeakSet[RevalidatingCache]" = weakref.WeakSet()
_REGISTRY_LOCK = threading.Lock()


def invalidate_all() -> None:
    """Drop every registered cache. For a write whose reach is not known."""
    with _REGISTRY_LOCK:
        caches = list(_REGISTRY)
    for cache in caches:
        cache.invalidate()


class RevalidatingCache:
    """Serve from memory, recheck behind the response, bound the staleness."""

    def __init__(self, name: str, *, fresh_for: float, max_served_age: float,
                 max_entries: int = 64, wait_timeout: float = 120.0,
                 clock: Callable[[], float] = time.monotonic,
                 spawn: Optional[Callable[[Callable[[], None], str], None]] = None):
        if not (0.0 <= fresh_for <= max_served_age):
            raise ValueError("fresh_for must be within max_served_age")
        if max_entries < 1:
            raise ValueError("max_entries must be positive")
        self.name = name
        self._fresh_for = float(fresh_for)
        self._max_served_age = float(max_served_age)
        self._max_entries = int(max_entries)
        self._wait_timeout = float(wait_timeout)
        self._clock = clock
        self._spawn = spawn or _spawn_thread
        self._lock = threading.Lock()
        self._entries: Dict[Hashable, _Entry] = {}
        self._flights: Dict[Hashable, _Flight] = {}
        self._epoch = 0
        self._counts = {"from_memory": 0, "waited": 0, "started": 0, "failed": 0}
        with _REGISTRY_LOCK:
            _REGISTRY.add(self)

    # ── serving ───────────────────────────────────────────────────────────── #

    def serve(self, key: Hashable, validate: Validate) -> Any:
        """Answer `key`, from memory where that is within the staleness bound.

        `validate` is handed the stored ``(value, token)`` or ``None``, and
        returns the same shape. It may return what it was given when the token
        says nothing has moved, which is what keeps a recheck cheaper than a
        rebuild. It must depend on nothing but `key`: a recheck already in
        flight answers whoever arrives next, so two callers of one key have to
        be asking the same question — which is why anything that changes the
        answer, the reader's identity included, belongs in the key.
        """
        with self._lock:
            entry = self._entries.get(key)
            age = None if entry is None else max(0.0, self._clock() - entry.validated_at)
            if entry is not None and age < self._fresh_for:
                self._counts["from_memory"] += 1
                return entry.value
            servable = entry is not None and age <= self._max_served_age
            stale_value = entry.value if servable else None
            flight = self._flights.get(key)
            ours = flight is None
            if ours:
                flight = self._flights[key] = _Flight(self._epoch)
                self._counts["started"] += 1
            self._counts["from_memory" if servable else "waited"] += 1

        if servable:
            # Zero round trips on the way out. The recheck, if this caller is
            # the one that claimed it, runs behind the response.
            if ours:
                self._behind_the_response(key, flight, validate)
            return stale_value
        if ours:
            self._validate(key, flight, validate)
        return flight.result(self._wait_timeout)

    def _behind_the_response(self, key: Hashable, flight: _Flight,
                             validate: Validate) -> None:
        try:
            self._spawn(lambda: self._validate(key, flight, validate),
                        f"{self.name}-revalidate")
        except Exception as error:  # noqa: BLE001 - the answer already went out
            # Nothing was rechecked, so nothing may be left claiming to be.
            with self._lock:
                self._flights.pop(key, None)
            flight.fail(error)
            logger.warning("%s could not start a revalidation: %s", self.name, error)

    def _validate(self, key: Hashable, flight: _Flight, validate: Validate) -> None:
        try:
            with self._lock:
                entry = self._entries.get(key)
            previous = None if entry is None else (entry.value, entry.token)
            value, token = validate(previous)
            with self._lock:
                self._flights.pop(key, None)
                if token is None:
                    # Unvalidated: answered now, never remembered.
                    self._entries.pop(key, None)
                elif flight.epoch == self._epoch:
                    self._entries[key] = _Entry(value, token, self._clock())
                    self._evict()
                # An epoch that moved means this process wrote while the
                # recheck was in flight, so what it read is already superseded.
                # Its caller still gets the answer; the next one rechecks.
            flight.finish(value, token)
        except BaseException as error:  # noqa: BLE001 - reported to the waiters
            with self._lock:
                self._flights.pop(key, None)
                self._counts["failed"] += 1
            # The entry is deliberately left as it stands. Its `validated_at`
            # does not advance, so continued failure ages it past
            # `max_served_age` and the next caller is told rather than served.
            flight.fail(error)
            logger.warning("%s revalidation failed for %r", self.name, key,
                           exc_info=error)

    def _evict(self) -> None:
        while len(self._entries) > self._max_entries:
            oldest = min(self._entries, key=lambda k: self._entries[k].validated_at)
            self._entries.pop(oldest, None)

    # ── invalidation and inspection ───────────────────────────────────────── #

    def invalidate(self, key: Optional[Hashable] = None) -> None:
        """Forget what is held, immediately, for one key or for every key."""
        with self._lock:
            self._epoch += 1
            if key is None:
                self._entries.clear()
            else:
                self._entries.pop(key, None)

    def stats(self) -> Dict[str, Any]:
        with self._lock:
            return {"entries": len(self._entries), "in_flight": len(self._flights),
                    **self._counts}


def _spawn_thread(work: Callable[[], None], name: str) -> None:
    threading.Thread(target=work, name=name, daemon=True).start()
