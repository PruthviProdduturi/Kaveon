"""Filling the hot reads before the first reader arrives.

Every hot read in Studio is computed on the first request that needs it, so
the first visit after a restart pays the whole cost and so does the first
visit after anything invalidates (defect D-27). The cost is not incidental:
the Engine answers any product listing in about half a second regardless of
how few records it holds (D-3, Engine-side), the home page's context banner
measured 2.1s cold, and a page makes several such reads. Nothing was warmed,
so somebody was always the one who paid.

This is the thing that pays instead. It is not a second cache: it drives the
caches that already serve these reads — ``revalidating_cache.warm`` on each
one, on an interval, for the principals who are going to read them — so a
reader arriving afterwards is answered from memory with no Engine round trip
at all.

Two caches are driven:

**The context banner** (``dlm.engine.warm_coverage``). Its cache already
exists and a hit already costs zero round trips; what was missing is anything
keeping an entry inside its freshness bound between one visit and the next.
Its ``max_served_age`` is 300s, so an interval under that is what makes a
visit ten minutes later as cheap as a reload.

**The Library listings** (``listing`` below) — charts, dashboards, datasets
and sources, which is what the Library's tabs and the home page's source
strip read. These had no cache at all, so this module provides one of the same
shape, and warms it. **Its readers are not yet wired up**: the four call sites
that would benefit — ``services/charts.list_charts``,
``services/dashboards.list_dashboards``, ``services/datasets.list_datasets``
and the ``sources`` listing in ``routers/data_sources`` and
``routers/catalog_sources`` — still call ``product_read_authority``
directly, and swapping them to ``warm_reads.listing`` is a one-line change in
files this change does not own. Two things must land with that swap, and are
not assumed here:

  * a write to one of these families must call ``invalidate`` (or
    ``revalidating_cache.invalidate_all``) so a reader sees their own edit at
    once rather than at the end of the staleness window. ``max_served_age``
    bounds only the writes *another* replica makes.
  * ``listing`` hands out a copy, because what it holds is shared between
    readers; a caller that sorted or decorated the list in place today would
    otherwise corrupt the entry for everybody.

Nothing here runs at import. The warmer is started by ``main.py`` after the
application is already serving, and stopped when it shuts down, so the test
suite — which never enters the application's lifespan except in the one case
that exits it immediately — starts no threads.
"""

from __future__ import annotations

import hashlib
import logging
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Tuple

from config import settings
from services import product_reads, revalidating_cache

logger = logging.getLogger(__name__)

# A principal is the pair every one of these caches is keyed by. It is a pair
# and not an email because role decides what the payload contains.
Principal = Tuple[str, str]


# ── Which principals are warmed, and what that does not cover ──────────────── #
#
# These payloads are visibility filtered and every cache here is keyed by actor
# and role, so warming is only useful — and only correct — for a real
# principal. Warming under an ambient admin identity and serving that answer
# to whoever asked next is deliberately not done: it would not be an
# optimisation, it would be one reader being shown another reader's content.
#
# Two sources, each honest about what it knows:
#
#   **Configured** — ``KAVEON_WARM_PRINCIPALS``, e.g.
#   ``"pruthvi@example.com:Admin, analyst@example.com:Analyst"``. This is the
#   only thing that can make the *first* visit after a restart warm, because
#   at that moment nothing has been seen yet. It is as honest as the operator's
#   list: a principal who is named but never reads costs one wasted cycle's
#   reads, and a principal who reads but is not named gets nothing from it.
#
#   **Recently seen** — a principal whose home page actually reached
#   ``dlm.engine.coverage`` is recorded by that call through
#   ``note_principal``, and warmed for ``PRINCIPAL_TTL`` afterwards. This is
#   not a session store and knows nothing about sessions: it is a bounded
#   in-process ring of the last ``MAX_PRINCIPALS`` pairs *this replica*
#   served. It does not survive a restart, it is not shared between replicas,
#   and a replica warms only who it has seen itself. So the limit is precise:
#   a reader who is neither configured nor seen still pays one cold read, and
#   is warm from then until they go quiet for ``PRINCIPAL_TTL``.
PRINCIPAL_TTL = 1800.0
MAX_PRINCIPALS = 8

_ROLES = ("Viewer", "Analyst", "Editor", "Admin")
_DEFAULT_ROLE = "Viewer"

_seen: "OrderedDict[Principal, float]" = OrderedDict()
_seen_lock = threading.Lock()


def note_principal(actor: str, role: str, *, now: Optional[float] = None) -> None:
    """Record that `actor` is reading, so what they read is kept warm.

    Called from the read path, so it must cost nothing and never raise: it is
    a bounded dictionary under a lock, and the oldest entry goes when the
    bound is reached.
    """
    if not actor or role not in _ROLES:
        return
    moment = time.monotonic() if now is None else now
    with _seen_lock:
        _seen.pop((actor, role), None)
        _seen[(actor, role)] = moment
        while len(_seen) > MAX_PRINCIPALS:
            _seen.popitem(last=False)


def forget_principals() -> None:
    """Drop the recently-seen ring. For a test, and for an operator reset."""
    with _seen_lock:
        _seen.clear()


def configured_principals() -> List[Principal]:
    """The principals an operator stated will read this installation.

    ``email:Role``, comma separated. An entry naming no role is taken as
    Viewer, which is the narrowest reading and therefore the safe default; an
    entry naming an unknown role is rejected rather than widened.
    """
    raw = (settings.KAVEON_WARM_PRINCIPALS or "").strip()
    if not raw:
        return []
    out: List[Principal] = []
    for item in raw.split(","):
        item = item.strip()
        if not item:
            continue
        actor, _, role = item.partition(":")
        actor, role = actor.strip(), (role.strip() or _DEFAULT_ROLE)
        if not actor or role not in _ROLES:
            logger.warning("Ignoring unreadable warm principal %r", item)
            continue
        if (actor, role) not in out:
            out.append((actor, role))
    return out


def principals(*, now: Optional[float] = None) -> List[Principal]:
    """Everybody worth warming for, configured first, without duplicates."""
    moment = time.monotonic() if now is None else now
    out = configured_principals()
    with _seen_lock:
        expired = [key for key, seen in _seen.items() if moment - seen > PRINCIPAL_TTL]
        for key in expired:
            _seen.pop(key, None)
        recent = list(_seen)
    for principal in recent:
        if principal not in out:
            out.append(principal)
    return out


# ── The Library listings, served from memory ───────────────────────────────── #

LISTING_FAMILIES = ("charts", "dashboards", "datasets", "sources")

# The windows, and why they are these. Inside FRESH_FOR a listing is served
# and nothing is started, which collapses the several reads one Library load
# makes into one. Past it the reader is still answered from memory and the
# recheck runs behind the response. Past MAX_SERVED_AGE the reader waits — the
# stated bound on how stale a listing can be, and what it bounds is a write by
# another replica, since a write here invalidates immediately. They match the
# context banner's deliberately: the two are read by the same page load and
# there is no reason for one to be stale longer than the other.
LISTING_FRESH_FOR = 10.0
LISTING_MAX_SERVED_AGE = 300.0

_LISTINGS = revalidating_cache.RevalidatingCache(
    "product-listings",
    fresh_for=LISTING_FRESH_FOR,
    max_served_age=LISTING_MAX_SERVED_AGE,
    # Four families for each of a handful of readers, with room to spare.
    max_entries=64,
)


def _listing_identity(documents: List[dict]) -> str:
    """What this listing says, as one token the next recheck can compare.

    Taken over each record's identity, the generation and modification it
    reports, and whether this reader has favorited it — not over the whole
    document, so a recheck that finds nothing moved keeps the answer it
    already holds rather than rebuilding it.

    What it rests on is that the store advances a record's generation on every
    write, which is what its compare-and-set commit does. The favorite is in
    here because it is the one field of a Library card that moves without the
    record moving: it comes from the caller's own favorites listing, which
    ``list_documents`` reads alongside the family.
    """
    digest = hashlib.sha256()
    for document in documents:
        digest.update("\0".join((
            str(document.get("id") or document.get("source_id") or ""),
            str(document.get("generation") or ""),
            str(document.get("updated_at") or document.get("modified_at") or ""),
            str(document.get("favorite") or ""),
        )).encode("utf-8"))
        digest.update(b"\x1e")
    return digest.hexdigest()


def _validated_listing(family: str, actor: str, role: str,
                       previous: Optional[tuple]) -> tuple:
    from services import product_read_authority
    documents = product_read_authority.list_documents(family, actor, role)
    token = _listing_identity(documents)
    if previous is not None and previous[1] == token:
        # Nothing in the family moved, so the answer already held describes it.
        return previous
    return documents, token


def listing(family: str, actor: str, role: str) -> List[dict]:
    """One visible product family for one principal, from memory where it can be.

    The copy is deliberate: the entry is shared between readers, and a caller
    that sorts or decorates its list in place must not be able to change what
    the next reader is served.
    """
    if family not in LISTING_FAMILIES:
        raise ValueError(f"{family} is not a warmed product listing")
    documents = _LISTINGS.serve(
        (family, actor, role),
        lambda previous: _validated_listing(family, actor, role, previous),
    )
    return [dict(document) for document in documents]


def invalidate() -> None:
    """Forget every held listing, for a write that has just superseded one.

    Deliberately not per family and not per reader: a listing is keyed by the
    reader it was filtered for, and who holds an entry is not enumerable from
    a write site, so narrowing this would silently leave some readers on the
    superseded answer. Dropping all four costs the next reader of each one
    listing, which is the right trade against showing somebody a chart they
    have just deleted.
    """
    _LISTINGS.invalidate()


# ── What a cycle warms ─────────────────────────────────────────────────────── #

NOT_APPLICABLE = "not applicable"
UNAVAILABLE = "unavailable"


@dataclass(frozen=True)
class Warmed:
    """One read, for one principal, and what warming it did."""

    task: str
    actor: str
    outcome: str
    detail: str = ""

    @property
    def failed(self) -> bool:
        return self.outcome == UNAVAILABLE


def _warm_coverage(actor: str, role: str) -> str:
    from dlm import engine
    return engine.warm_coverage(actor, role)


def _warm_listing(family: str) -> Callable[[str, str], str]:
    def warm(actor: str, role: str) -> str:
        from services import product_read_authority
        if not product_read_authority.enabled(family):
            # The family still reads from PostgreSQL on this deployment, where
            # a listing is a local query rather than an Engine round trip.
            return NOT_APPLICABLE
        return _LISTINGS.warm(
            (family, actor, role),
            lambda previous: _validated_listing(family, actor, role, previous),
        )
    return warm


def tasks() -> "OrderedDict[str, Callable[[str, str], str]]":
    """The hot reads a cycle fills, named for the log line.

    Established from what the pages read, not from what is convenient: the home
    page's banner (``/api/v1/dlm/coverage``) and its source strip
    (``/api/v1/data-sources/list`` → the ``sources`` family), and the Library's
    four tabs (``/api/v1/charts``, ``/api/v1/dashboards``, ``/api/v1/datasets``
    and the dataset shapes the composer reads through the same listing).
    """
    out: "OrderedDict[str, Callable[[str, str], str]]" = OrderedDict()
    out["dlm.coverage"] = _warm_coverage
    for family in LISTING_FAMILIES:
        out[family] = _warm_listing(family)
    return out


@dataclass
class Cycle:
    """What one pass filled and what it could not, as one line."""

    principals: int = 0
    filled: int = 0
    fresh: int = 0
    unavailable: int = 0
    skipped: int = 0
    not_applicable: int = 0
    seconds: float = 0.0
    results: List[Warmed] = field(default_factory=list)

    def line(self) -> str:
        if self.skipped:
            return "warm cycle skipped: one was already running"
        head = (f"warm cycle: principals={self.principals} filled={self.filled} "
                f"fresh={self.fresh} unreachable={self.unavailable} "
                f"inapplicable={self.not_applicable} in {self.seconds:.2f}s")
        failures = [f"{item.task}/{item.actor}: {item.detail}"
                    for item in self.results if item.failed]
        if failures:
            # The one place a cycle says anything beyond its counts: what it
            # could not warm, and why, which is the whole diagnostic value of
            # running it at all.
            return head + "; could not warm " + ", ".join(sorted(failures))
        return head


_cycle_lock = threading.Lock()


def run_cycle() -> Cycle:
    """Warm every task for every principal once. Never raises.

    One cycle at a time, process-wide: a cycle that outruns its interval, or
    an operator running one by hand, must not double the Engine's load or have
    two passes validating the same key. The second caller is told it was
    skipped rather than made to wait.
    """
    if not _cycle_lock.acquire(blocking=False):
        return Cycle(skipped=1)
    started = time.monotonic()
    cycle = Cycle()
    try:
        who = principals()
        cycle.principals = len(who)
        warmers = tasks()
        for actor, role in who:
            # Independent reads, so they overlap — the same bounded fan-out the
            # request path uses, which is what keeps a cycle from becoming a
            # burst of connections to the coordinator.
            for result in product_reads.gather(*[
                _attempt(name, warm, actor, role) for name, warm in warmers.items()
            ]):
                cycle.results.append(result)
                if result.outcome == UNAVAILABLE:
                    cycle.unavailable += 1
                elif result.outcome == NOT_APPLICABLE:
                    cycle.not_applicable += 1
                elif result.outcome == revalidating_cache.RevalidatingCache.WARM_FRESH:
                    cycle.fresh += 1
                else:
                    cycle.filled += 1
    finally:
        cycle.seconds = time.monotonic() - started
        _cycle_lock.release()
    return cycle


def _attempt(name: str, warm: Callable[[str, str], str],
             actor: str, role: str) -> Callable[[], Warmed]:
    """One warm, with its failure turned into a reportable outcome.

    The Engine is scaled to zero for long stretches by the benchmark campaigns,
    so a read failing is the ordinary case rather than the exception: it is
    recorded against the task that could not be warmed and the cycle carries
    on. Nothing already held is discarded — the cache leaves a stale entry
    exactly as it stands when a validation fails, which is what makes a cycle
    during an outage safe to run at all.
    """
    def attempt() -> Warmed:
        try:
            return Warmed(name, actor, warm(actor, role))
        except BaseException as error:  # noqa: BLE001 - reported, not raised
            # The status matters as much as the text: 503 is the Engine scaled
            # to zero and 504 is a coordinator that is up but not keeping up,
            # and an operator reading one line needs to be able to tell them
            # apart without going to the Engine.
            status = getattr(error, "status_code", None)
            detail = getattr(error, "detail", None) or str(error)
            named = type(error).__name__ + ("" if status is None else f" {status}")
            return Warmed(name, actor, UNAVAILABLE, f"{named} {detail}".strip())
    return attempt


# ── The loop ───────────────────────────────────────────────────────────────── #


@dataclass
class Handle:
    stop: threading.Event
    thread: threading.Thread


_start_lock = threading.Lock()
_handle: Optional[Handle] = None


def _interval() -> float:
    """How often a cycle runs, bounded by what it is keeping inside its window.

    An interval at or past ``max_served_age`` would let an entry age out
    between cycles, which is the cost this exists to remove, so it is clamped
    to a fraction of the tighter of the two bounds rather than taken on trust.
    """
    bound = min(LISTING_MAX_SERVED_AGE, _coverage_max_served_age()) / 2.0
    configured = float(settings.KAVEON_WARM_INTERVAL_SECONDS)
    return max(1.0, min(configured, bound))


def _coverage_max_served_age() -> float:
    from dlm import engine
    return float(engine.COVERAGE_MAX_SERVED_AGE)


def _loop(stop: threading.Event, *, delay: float, interval: float,
          sleep: Optional[Callable[[float], bool]] = None) -> None:
    """Warm on an interval until told to stop, whatever any cycle does.

    The first pass waits: nothing may run during import or compete with the
    application's own startup, and readiness must not depend on the Engine
    being up. ``sleep`` returning True means stop — it is the stop event's own
    wait, so a shutdown is immediate rather than at the end of an interval.
    """
    wait = sleep or stop.wait
    if wait(delay):
        return
    while True:
        try:
            logger.info("%s", run_cycle().line())
        except BaseException:  # noqa: BLE001 - a warmer may never end the process
            logger.exception("Read warming cycle failed")
        if wait(interval):
            return


def start() -> Optional[Handle]:
    """Start warming shortly after startup, unless it is switched off.

    Returns the handle to stop it with, or None when ``KAVEON_READ_WARMER_ENABLED``
    is off. Starting twice is not an error and does not start a second loop:
    the running one is returned.
    """
    if not settings.KAVEON_READ_WARMER_ENABLED:
        return None
    global _handle
    with _start_lock:
        if _handle is not None and _handle.thread.is_alive():
            return _handle
        stop = threading.Event()
        delay = max(0.0, float(settings.KAVEON_WARM_START_DELAY_SECONDS))
        interval = _interval()
        thread = threading.Thread(
            target=_loop, args=(stop,),
            kwargs={"delay": delay, "interval": interval},
            name="read-warmer", daemon=True,
        )
        _handle = Handle(stop, thread)
        thread.start()
        logger.info("Read warming started: first cycle in %.0fs, then every %.0fs",
                    delay, interval)
        return _handle


def stop(handle: Optional[Handle] = None, *, timeout: float = 5.0) -> None:
    """Stop warming and wait for the cycle in flight to finish."""
    global _handle
    with _start_lock:
        target = handle or _handle
        if target is None:
            return
        if handle is None or handle is _handle:
            _handle = None
    target.stop.set()
    target.thread.join(timeout=timeout)


def status() -> Dict[str, object]:
    """What is warm, for how many principals, and whether the loop is running."""
    return {
        "running": _handle is not None and _handle.thread.is_alive(),
        "enabled": bool(settings.KAVEON_READ_WARMER_ENABLED),
        "interval_seconds": _interval(),
        "principals": len(principals()),
        "listings": _LISTINGS.stats(),
    }
