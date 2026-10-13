"""Scheduled recompile of stale DLM context.

The numbers Studio reports about a dataset — its row count, its date range,
which values can be answered without a scan — are recorded when the context is
compiled, not measured when they are read. So when a table grows, nothing in
the product catalog changes and nothing says the recorded numbers are now
wrong: a table taking a daily append reports yesterday's count until something
recompiles it.

`dlm.engine.check_freshness` already detects exactly this. An Engine-backed
dataset compares the `identity_sha256` the artifact recorded against the
table's current one, and a Delta append changes that identity, so staleness is
a fact rather than an estimate. What was missing is anything acting on it
unattended: the in-process sweep loop (`engine._start_sweep_loop`) returns
immediately whenever the PostgreSQL-free runtime is requested, and the sweep
itself needs a caller identity to read compiled artifacts at all, which a
thread started at import has none of. On this installation that left context
to be refreshed only by someone pressing a button.

This runs as a scheduled job rather than in the API, for two reasons. The API
serves requests, and compiling the context for a half-billion-row table is not
a request-sized amount of work: one such rebuild exceeded the API's memory
limit and the kernel killed the pod mid-generation, which leaves the dataset
with its old context already invalidated — removing context rather than
declining to rebuild it. And rebuilds here are sequential and in-process, so
one dataset is compiled at a time against a bounded limit, and the job's exit
means the work is actually finished rather than handed to a daemon thread that
dies with the process.
"""

from __future__ import annotations

import json
import logging
import os
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)

def sweep_identity() -> Optional[str]:
    """The principal the scheduled sweep reads and writes as.

    Compiled context is owned content: listing it needs an identity, and a
    rebuild writes a new artifact under one. There is no ambient
    administrator, so the schedule is given a principal explicitly rather
    than inheriting one, and a job with none refuses rather than silently
    checking nothing.
    """
    return os.getenv("KAVEON_DLM_SWEEP_ACTOR", "").strip() or None


def _stale_datasets(engine) -> tuple[int, List[Dict[str, Any]]]:
    """Every compiled dataset whose source has moved since it was compiled.

    Freshness is read under the sweep's own identity, and the whole listing is
    taken before the first rebuild: a rebuild publishes a new artifact, and
    enumerating as we go would then be deciding what to check from a catalog
    the job is itself changing.
    """
    examined = 0
    stale: List[Dict[str, Any]] = []
    for artifact in engine._serving_artifacts():
        dataset_id = str((artifact or {}).get("dataset_id") or "")
        if not dataset_id:
            continue
        examined += 1
        try:
            freshness = engine.check_freshness(dataset_id)
        except Exception:
            # One unreadable dataset is not a reason to leave the others
            # stale, and the reason it was unreadable belongs in the log.
            logger.exception("freshness check failed for dataset %s", dataset_id)
            continue
        if freshness.get("fresh") or freshness.get("recommendation") != "rebuild":
            continue
        stale.append({"dataset_id": dataset_id, "score": freshness.get("score"),
                      "reason": freshness.get("reason")})
    return examined, stale


def sweep(actor: str, role: str = "Admin") -> Dict[str, Any]:
    """Recompile every dataset whose source has changed. Returns what happened.

    Rebuilds run one at a time and to completion. The in-process sweep started
    a daemon thread per stale dataset, which is the right shape for a server
    that stays up and the wrong one for a job: the process would exit with the
    work abandoned halfway, having already invalidated the context it was
    replacing.
    """
    from dlm import engine

    state = engine._RetirementServingState(actor, role)
    token = engine._RETIREMENT_SERVING.set(state)
    try:
        examined, stale = _stale_datasets(engine)
    finally:
        engine._RETIREMENT_SERVING.reset(token)

    rebuilt: List[str] = []
    failed: List[Dict[str, str]] = []
    for entry in stale:
        dataset_id = entry["dataset_id"]
        logger.info("recompiling dataset %s (freshness %s)", dataset_id, entry.get("score"))
        try:
            engine.generate_dlm(dataset_id, force=True, actor=actor, role=role)
        except Exception as error:
            # Recorded and carried on. A dataset that cannot compile leaves
            # its previous context in place, so the failure costs freshness
            # rather than availability — and the next run tries again.
            logger.exception("recompile failed for dataset %s", dataset_id)
            failed.append({"dataset_id": dataset_id, "error": str(error)})
            continue
        rebuilt.append(dataset_id)

    return {"checked": examined,
            "stale": [entry["dataset_id"] for entry in stale],
            "rebuilt": rebuilt, "failed": failed}


def main() -> int:
    """Entry point for the scheduled sweep.

    Prints one line of what happened. A run that found every context current
    is a success and says so, because a job that only reported failure would
    make "nothing to do" indistinguishable from "never ran".
    """
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
    # httpx logs a line per request at INFO, and a sweep makes one request per
    # table it checks — which buried the one line the job exists to print under
    # a page of round trips. The transport's failures still surface.
    logging.getLogger("httpx").setLevel(logging.WARNING)
    actor = sweep_identity()
    if not actor:
        print("KAVEON_DLM_SWEEP_ACTOR is not set; refusing to sweep as nobody")
        return 2

    result = sweep(actor)
    if not result["stale"]:
        print(f"context current: {result['checked']} datasets checked, nothing to recompile")
        return 0
    print(f"recompiled {len(result['rebuilt'])} of {len(result['stale'])} stale datasets: "
          f"{json.dumps(result, default=str)}")
    # A failure is reported as one. The CronJob's backoff is what retries it,
    # and a job that exited zero after failing to recompile would make a
    # permanently stale dataset invisible.
    return 1 if result["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
