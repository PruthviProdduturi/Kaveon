"""The words readers used that their dataset could not place.

A dataset arrives with its structure derived -- dimensions, measures, time
grain, value index, all from the Engine's statistics -- and with generic
business English for vocabulary, from a seed lexicon of 56 heads. What it
cannot derive is the vocabulary of the organisation reading it: that
`rev_net_usd` is "the number", that "churn" here means cancelled accounts,
the product names, the acronyms.

That gap used to be invisible and harmful at once, because an unmapped word
was answered with whatever measure the default was. It now refuses by name,
which makes the gap visible -- and a refusal is a better signal than any
upfront modelling exercise, because a real reader asked a real question with
it. This ledger collects them so curation becomes a short queue driven by
demand instead of a guess made in advance.

**What is durable and what is not.** A resolution is durable: mapping a word
to a measure writes a curated alias through the dataset's curation record,
which is where every other human statement about a dataset already lives, and
takes effect on the next question. The *observations* in this module are held
in this process, bounded, and are lost on a restart. That is a deliberate
trade rather than an oversight: a durable, organisation-wide ledger wants a
`vocabulary_gap` product record, and the Engine's transaction API validates
each record kind by name -- adding one is the Engine owner's change, not this
module's. Until then the signal regenerates itself, because the next reader to
ask the question records it again. Nothing is silently dropped: the API says
the ledger is process-local when it answers.

**What it does not see.** A word is recorded against the dataset the question
routed to, so a question that routes nowhere -- `reason: "no_dataset"`, where
nothing in the question matched any dataset's vocabulary -- leaves no entry.
There is no element to map such a word onto, which makes it a different
finding: nobody can answer anything about that subject, rather than this
dataset not knowing a word for something it holds.
"""

import threading
import time
from typing import Any, Dict, Iterable, List, Optional

# A refusal is cheap and repetitive, so the ledger is capped rather than
# trusted to stay small. The caps bound memory on a public deployment where
# every unanswerable question would otherwise be remembered forever; the
# oldest-seen term is evicted first, which is the one least likely to be
# curated.
MAX_TERMS = 500
MAX_EXAMPLES = 3
MAX_EXAMPLE_LENGTH = 200

_LOCK = threading.Lock()
# (dataset_id, term) -> {"count", "first_seen", "last_seen", "examples", "askers"}
_GAPS: Dict[tuple, Dict[str, Any]] = {}


def record(dataset_id: Any, term: str, question: str = "",
           actor: Optional[str] = None) -> None:
    """Note that `term` made a question about this dataset unanswerable.

    Never raises: this runs on the refusal path, and a question that could not
    be answered must still be refused cleanly if the ledger cannot take it.
    """
    try:
        key = (str(dataset_id or ""), str(term or "").strip().lower())
        if not key[0] or not key[1]:
            return
        now = time.time()
        example = str(question or "").strip()[:MAX_EXAMPLE_LENGTH]
        with _LOCK:
            entry = _GAPS.get(key)
            if entry is None:
                if len(_GAPS) >= MAX_TERMS:
                    oldest = min(_GAPS, key=lambda k: _GAPS[k]["last_seen"])
                    _GAPS.pop(oldest, None)
                entry = {"count": 0, "first_seen": now, "last_seen": now,
                         "examples": [], "askers": set()}
                _GAPS[key] = entry
            entry["count"] += 1
            entry["last_seen"] = now
            if example and example not in entry["examples"]:
                entry["examples"] = (entry["examples"] + [example])[:MAX_EXAMPLES]
            if actor:
                entry["askers"].add(str(actor).lower())
    except Exception:  # pragma: no cover - the refusal matters more than the note
        pass


def forget(dataset_id: Any, term: str) -> bool:
    """Drop a term from the ledger. Called once it is mapped or dismissed --
    both are answers, and a queue that keeps answered items is a list."""
    key = (str(dataset_id or ""), str(term or "").strip().lower())
    with _LOCK:
        return _GAPS.pop(key, None) is not None


def forget_dataset(dataset_id: Any) -> int:
    """Drop every term for a dataset, for when the dataset itself is gone."""
    prefix = str(dataset_id or "")
    with _LOCK:
        keys = [k for k in _GAPS if k[0] == prefix]
        for key in keys:
            _GAPS.pop(key, None)
    return len(keys)


def gaps(dataset_ids: Optional[Iterable[Any]] = None,
         dismissed: Optional[Dict[str, Iterable[str]]] = None) -> List[Dict[str, Any]]:
    """The ledger, most-asked first, as rows.

    `dataset_ids` restricts it to datasets the caller can see -- the questions
    people ask are as visibility-bound as the datasets they ask them of.
    `dismissed` drops terms a curator has already judged to be noise, per
    dataset; a dismissal is durable and this ledger is not, so a term is
    filtered on read rather than trusted to stay deleted.
    """
    allowed = None if dataset_ids is None else {str(i) for i in dataset_ids}
    skip = {str(k): {str(t).lower() for t in (v or ())}
            for k, v in (dismissed or {}).items()}
    rows: List[Dict[str, Any]] = []
    with _LOCK:
        for (dataset_id, term), entry in _GAPS.items():
            if allowed is not None and dataset_id not in allowed:
                continue
            if term in skip.get(dataset_id, ()):
                continue
            rows.append({
                "dataset_id": dataset_id,
                "term": term,
                "count": entry["count"],
                "askers": len(entry["askers"]),
                "first_seen": entry["first_seen"],
                "last_seen": entry["last_seen"],
                "examples": list(entry["examples"]),
            })
    # Most asked first, and the most recent of equals: the word worth mapping
    # is the one being asked most and still being asked.
    rows.sort(key=lambda row: (-row["count"], -row["last_seen"]))
    return rows


def clear() -> None:
    """Empty the ledger. For tests, and for an operator who wants to start the
    queue again from the questions asked after now."""
    with _LOCK:
        _GAPS.clear()
