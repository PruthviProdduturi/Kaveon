"""Builds a table's cube, out of process.

A cube lets the Engine answer a grouped aggregate over a table's declared
shape without scanning it. Nothing builds one implicitly: the freshness sweep
recompiles DLM *context* when a source changes, which is a different thing, so
a table can have current statistics, current context, and still scan half a
billion rows for every chart.

This runs as a job rather than from a session or the API for a reason learned
twice in one day. A build over a 10-axis shape issues one statement per
grouping — ten single axes plus the forty-five pairs the cube holds — so it
runs for tens of minutes. Both attempts made over `kubectl exec` died when a
deploy replaced the API pod underneath them: the connection holding the
statement went away and the coordinator cancelled it, losing about ten
completed groupings each time. Deploy-on-push is deliberate here until the
platform is stable, so anything that runs for tens of minutes has to own its
own pod.

The statement carries `engine_bridge.MAINTENANCE_CLIENT_TAG`, which the
coordinator's resource groups select into the maintenance group, so a build
cannot take the slots interactive queries need. Without that group a build
takes all four of the default group's slots and a chart is refused outright
rather than merely slowed.
"""

from __future__ import annotations

import logging
import os
import time
from typing import Optional, Tuple

logger = logging.getLogger(__name__)


def target() -> Optional[Tuple[str, str, str]]:
    """The table to build, as `catalog.schema.table`.

    Named explicitly rather than discovered. A job that went looking for
    tables worth cubing would decide, unattended, to spend tens of minutes of
    cluster time on one — and the choice of which tables carry a cube is a
    modelling decision, recorded as the table's shape, not something to infer
    from size.
    """
    raw = (os.getenv("KAVEON_CUBE_TARGET") or "").strip()
    if not raw:
        return None
    parts = [part.strip() for part in raw.split(".")]
    if len(parts) != 3 or not all(parts):
        return None
    return parts[0], parts[1], parts[2]


def actor() -> Optional[str]:
    """Who the build runs as. `ANALYZE` is an Engine admin operation and the
    catalog read that precedes it answers for a verified principal, so the job
    is given one explicitly rather than inheriting an ambient administrator."""
    return (os.getenv("KAVEON_CUBE_ACTOR") or "").strip() or None


def build(catalog: str, schema: str, table: str, principal: str) -> dict:
    """Build the cube, returning what happened.

    The shape is checked first. `ANALYZE … WITH (cube = true)` over a table
    that declares none has nothing to build over, and the Engine says so after
    the caller has already committed to the wait — so the refusal is raised
    here, before any scan, with the same message the interactive route gives.
    """
    from fastapi import HTTPException  # noqa: F401  (surfaced by the bridge)

    from services import engine_bridge

    table_id = f"table:{catalog}:{schema}:{table}"
    definition = engine_bridge.table_definition_by_id(table_id, principal, "Admin")
    if not isinstance(definition, dict):
        return {"ok": False, "reason": f"{table_id} is not a table this catalog holds"}
    shape = definition.get("shape") or {}
    dimensions = shape.get("dimensions") or []
    if not dimensions:
        return {"ok": False, "reason": (
            f"{catalog}.{schema}.{table} declares no shape, so there is nothing "
            f"for a cube to be built over")}

    # One grouping per axis plus the pairs the cube holds, which is what makes
    # this long rather than slow; logged so a reader of the job's output knows
    # what it is waiting for instead of watching silence.
    axes = len(dimensions)
    logger.info("building a cube over %s.%s.%s: %d axes, %d pairs",
                catalog, schema, table, axes, axes * (axes - 1) // 2)

    started = time.monotonic()
    statement, result = engine_bridge.analyze_table(
        catalog, schema, table, principal, "Admin", cube=True)
    columns = [column.get("name") if isinstance(column, dict) else str(column)
               for column in (result.get("columns") or [])]
    rows = result.get("data") or []
    summary = dict(zip(columns, rows[0])) if rows and isinstance(rows[0], list) else {}
    return {"ok": True, "statement": statement, "seconds": round(time.monotonic() - started, 1),
            "axes": axes, "summary": summary}


def main() -> int:
    """Entry point. One line of what happened, success or failure."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
    # The build makes one request per grouping; at INFO httpx prints a line for
    # each, which buries the result this job exists to report. Its failures
    # still surface.
    logging.getLogger("httpx").setLevel(logging.WARNING)

    principal = actor()
    if not principal:
        print("KAVEON_CUBE_ACTOR is not set; refusing to build as nobody")
        return 2
    chosen = target()
    if not chosen:
        print("KAVEON_CUBE_TARGET is not set to catalog.schema.table; refusing to guess")
        return 2

    catalog, schema, table = chosen
    try:
        outcome = build(catalog, schema, table, principal)
    except Exception as error:  # noqa: BLE001 - the message is the whole report
        # A refusal from the Engine is the thing the operator has to act on,
        # so it is printed rather than reduced to a traceback and an exit code.
        print(f"cube build failed for {catalog}.{schema}.{table}: "
              f"{type(error).__name__}: {str(error)[:600]}")
        return 1

    if not outcome["ok"]:
        print(f"cube not built: {outcome['reason']}")
        return 1
    print(f"cube built over {catalog}.{schema}.{table} in {outcome['seconds']}s "
          f"({outcome['axes']} axes): {outcome['summary']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
