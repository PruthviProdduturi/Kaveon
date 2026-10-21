"""Keep an owner's share of a growing family under a bound.

Three families accumulate a record per action rather than per decision, and
each arrived at its own answer. Query history sweeps the oldest away; chat
refuses the next message and asks the person to delete a conversation; activity
had no answer at all, which is why it was the one family with no bound and the
one that could fill a catalogue on its own.

Refusing is right where the records are the person's own work and losing the
oldest silently would be a surprise -- a conversation should not evaporate.
It is wrong for a record of what happened: refusing to record an action would
fail the action, and nobody asked for the ten-thousandth entry in an activity
feed to be kept forever.

So this is the sweeping half, written once. It is deliberately simpler than
the machinery in `query_history`, which carries a cached population count and
a background thread because it sits in front of every statement a person runs.
Activity is written when something is created or changed, which is orders of
magnitude rarer, and paying for a listing on those writes is affordable where
paying for it on every query was not.
"""
from __future__ import annotations

import logging
import threading
from dataclasses import dataclass
from typing import Callable

from services import product_store

_LOG = logging.getLogger(__name__)

# One transaction carries at most a hundred mutations, and a sweep's
# transaction holds nothing else, so it takes the whole allowance. Clearing
# more than the overage leaves the writes that follow far enough below the
# bound that they schedule nothing.
MAX_DELETE_FANOUT = 100

# Owners with a sweep already in flight. A burst of writes from one person --
# a dashboard saving a dozen charts -- would otherwise each see the same
# over-bound population and start their own sweep, and those sweeps would then
# race for the same records' revisions and all but one would fail.
_IN_FLIGHT: set[tuple[str, str]] = set()
_IN_FLIGHT_LOCK = threading.Lock()


@dataclass(frozen=True)
class RetentionPolicy:
    """How much of one family an owner may keep, and how to age it."""

    kind: product_store.ProductKind
    #: Records one owner may hold. Must sit below the listing ceiling with room
    #: for the writes that arrive before a sweep finishes, or an owner at the
    #: bound cannot be listed and therefore cannot be swept back under it.
    cap: int
    #: How many records a listing asks for. Above the cap, so the sweep can see
    #: the overage it has to clear.
    list_limit: int
    #: The document field a record is aged by, oldest first.
    ordered_by: str
    #: The document field naming the owner, checked before anything is deleted.
    owner_field: str

    def __post_init__(self) -> None:
        if self.cap >= self.list_limit:
            raise ValueError(
                f"{self.kind} retention cap {self.cap} is not below its listing limit "
                f"{self.list_limit}; an owner at the cap could not be listed, and the sweep "
                "that would bring them back under it reads the same listing"
            )


def _claim(policy: RetentionPolicy, owner: str) -> bool:
    with _IN_FLIGHT_LOCK:
        key = (policy.kind, owner)
        if key in _IN_FLIGHT:
            return False
        _IN_FLIGHT.add(key)
        return True


def _release(policy: RetentionPolicy, owner: str) -> None:
    with _IN_FLIGHT_LOCK:
        _IN_FLIGHT.discard((policy.kind, owner))


def sweep(policy: RetentionPolicy, owner: str) -> int:
    """Delete *owner*'s oldest records past the cap. Returns how many went.

    Reads through the capped listing rather than the refusing one, because an
    owner who is already over the bound is precisely the owner this exists to
    help, and refusing to list them would make the problem permanent.
    """
    records, truncated = product_store.list_records_capped(
        policy.kind, owner, "Admin", limit=policy.list_limit)
    over = len(records) - policy.cap
    if over <= 0 and not truncated:
        return 0
    # Truncated means the listing stopped early, so the population is a floor
    # rather than a count. Clearing a full batch is then the right move: it is
    # known to be over, and the next write schedules the next sweep.
    take = min(max(over, 0) + MAX_DELETE_FANOUT, MAX_DELETE_FANOUT) if truncated \
        else min(over, MAX_DELETE_FANOUT)

    def ordering(record: dict):
        document = record.get("document") or {}
        return (str(document.get(policy.ordered_by) or ""), str(document.get("id") or ""))

    mutations = []
    for oldest in sorted(records, key=ordering)[:take]:
        revision = oldest.get("revision")
        document = oldest.get("document")
        # A record whose owner does not match, or whose revision is not a
        # revision, means the listing is not what it claims to be. Deleting on
        # that basis would be deleting someone else's record.
        if type(revision) is not int or revision < 1 or not isinstance(document, dict) \
                or document.get(policy.owner_field) != owner:
            raise RuntimeError(f"KaveonDB {policy.kind} retention state is invalid")
        mutations.append(product_store.ProductMutation(
            "delete", policy.kind, str(document["id"]), expected_revision=revision))
    if not mutations:
        return 0
    product_store.transact(mutations, owner, "Analyst")
    return len(mutations)


def schedule(policy: RetentionPolicy, owner: str,
             *, spawn: Callable[..., threading.Thread] | None = None) -> None:
    """Sweep *owner* in the background, at most one sweep at a time.

    The caller has already done its work and been answered; retention is a
    bound rather than an invariant, so an owner sitting a little over it until
    this finishes is correct, and a person waiting on it is not.
    """
    if not _claim(policy, owner):
        return

    def run() -> None:
        try:
            sweep(policy, owner)
        except Exception as error:
            # The owner stays over the bound and the next write schedules
            # another sweep. A failure here must never reach the operation that
            # triggered it, which has already succeeded and been reported.
            _LOG.warning("product_retention_sweep_failed kind=%s type=%s status=%s",
                         policy.kind, type(error).__name__,
                         getattr(error, "status_code", None))
        finally:
            _release(policy, owner)

    factory = spawn or threading.Thread
    try:
        factory(target=run, name=f"{policy.kind}-retention", daemon=True).start()
    except RuntimeError:
        # Threads unavailable during interpreter shutdown. Running it here is
        # slower than the caller would like but leaves the claim released and
        # retention working, where returning would hold the claim forever and
        # stop this owner ever being swept again.
        run()
