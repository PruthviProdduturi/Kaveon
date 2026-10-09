"""Read-only Engine operations console, served through the platform bridge.

Studio is the only front door. The browser never holds an Engine credential:
each request carries the verified Kaveon principal and role to the Engine over
the server-side bridge, and the Engine applies its own ownership scoping to
query history. Nothing here mutates Engine state.
"""
import time

from fastapi import APIRouter, Depends, HTTPException, Response

from middleware import demo
from middleware.auth import UserContext
from middleware.permissions import require_min_role
from routers import lab
from services import engine_bridge, product_store

router = APIRouter(tags=["engine-console"])


@router.get("/engine/quota")
def engine_quota(ctx: UserContext = Depends(require_min_role("Viewer"))):
    """The caller's demo posture: whether the platform is read-only for them
    (`read_only`), and their live-read quota on the coordinator (`quota`,
    null when no quota applies to them — an Admin, a self-hosted install, or
    a coordinator without the Engine integration). Never an error for a
    missing Engine: the Studio shows the counter only when there is one."""
    read_only = demo.enabled() and ctx.role != "Admin"
    try:
        engine = engine_bridge.quota(ctx.email, ctx.role)
    except HTTPException as error:
        if error.status_code in {502, 503, 504}:
            return {"demo": {"read_only": read_only, "engine": False}, "quota": None}
        raise
    quota = engine.get("quota")
    return {"demo": {"read_only": read_only, "engine": bool(engine["demo"].get("enabled"))},
            "quota": quota if isinstance(quota, dict) else None}


@router.get("/engine/console/cluster")
def console_cluster(ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.cluster(ctx.email, ctx.role)
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid cluster record")
    return result


@router.get("/engine/console/queries")
def console_queries(ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.queries(ctx.email, ctx.role)
    if not isinstance(result, list):
        raise HTTPException(502, "Engine returned an invalid query listing")
    return result


@router.get("/engine/console/queries/{query_id}")
def console_query(query_id: str, ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.query(query_id, ctx.email, ctx.role)
    if result is None:
        raise HTTPException(404, "Query not found")
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid query record")
    return result


@router.get("/engine/console/statistics")
def console_statistics(ctx: UserContext = Depends(require_min_role("Admin"))):
    result = engine_bridge.statistics(ctx.email, ctx.role)
    if not isinstance(result, dict) or not isinstance(result.get("statistics"), list):
        raise HTTPException(502, "Engine returned invalid statistics diagnostics")
    return result


@router.post("/engine/console/statistics/qualify")
def qualify_native_statistics(ctx: UserContext = Depends(require_min_role("Admin"))):
    if not engine_bridge.native_analyze_supported():
        raise HTTPException(409, "Connected Engine does not advertise native ANALYZE")
    result = engine_bridge.execute(
        'ANALYZE "ai_benchmarks"."leaderboard"', "OpenSource",
        ctx.email, ctx.role, schema="ai_benchmarks",
    )
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid ANALYZE result")
    return {"capability": {"native_analyze": True}, "query": result}


# ── KaveonDB's own catalog ───────────────────────────────────────────────────
# The Engine registers one built-in catalog for the platform's own records.
# Its SQL identifier is `kaveon` and its control-plane schema is `product`;
# "KaveonDB" is the product's name for it and appears only in Studio, never in
# a statement. Every record under it is written through the Engine's
# transaction boundary (services.product_store.transact) and read back from a
# pinned durable snapshot, so a reading of it is a reading of one committed
# generation rather than a scan.
SYSTEM_CATALOG = "kaveon"
SYSTEM_SCHEMA = "product"

# The control plane's own families, in reading order: the content a reader
# came for, then the semantic layer, then the ledgers that record use. Each
# entry is (record kind, table name) and the pair is fixed in
# services.product_store._PRODUCT_TABLES — never caller-supplied.
_SYSTEM_TABLES: tuple[tuple[str, str], ...] = (
    ("dataset", "datasets"),
    ("chart", "charts"),
    ("dashboard", "dashboards"),
    ("saved_query", "saved_queries"),
    ("source", "sources"),
    ("dlm_definition", "dlm_definitions"),
    ("dlm_run", "dlm_runs"),
    ("chat_session", "chat_sessions"),
    ("chat_message", "chat_messages"),
    ("favorite", "favorites"),
    ("user_theme", "user_themes"),
    ("user_recent", "user_recents"),
    ("query_history", "query_history"),
    ("activity", "activity"),
)

# The Engine answers a product family as its documents, so a cardinality
# costs a read of every record in it. Counting is therefore asked for
# explicitly (`counts=true`) and the structure is free, which lets Studio draw
# the schema at once and fill the counts afterwards. The reads are sequential
# on purpose: the coordinator serializes a product listing against the durable
# snapshot, so fanning fourteen families out concurrently made one reading of
# the control plane five times slower rather than faster.
#
# A counted reading is held briefly, keyed by caller, because the Engine
# answers a product listing for the verified principal.
_SYSTEM_CATALOG_TTL_SECONDS = 15
_SYSTEM_CATALOG_CACHE: dict = {}


@router.get("/engine/console/system-catalog")
def system_catalog(response: Response, counts: bool = False, refresh: bool = False,
                   ctx: UserContext = Depends(require_min_role("Admin"))):
    """What KaveonDB's own `product` schema holds, family by family.

    Studio shows this beside the data catalogs so an administrator can see the
    platform's control plane without a credential of their own and without a
    statement: the Engine has no SQL schema called `product` — the name is a
    transaction facade its parser resolves to the product manifest — so these
    tables cannot be scanned, only read through this boundary.

    Each entry carries the real qualified identifier, so a reader never has to
    guess it from the product name. `counts=true` adds a bounded record count
    per family, read from one pinned snapshot; without it `records` is null,
    which is how a structure-only reading says it counted nothing rather than
    that a family is empty. `truncated` marks a family counted only as far as
    the bound, and a family the Engine refuses individually carries its
    message and leaves the rest of the reading intact.
    """
    response.headers.update(lab.NO_CACHE)
    if not counts:
        return _system_catalog_document(
            [_system_row(entry) for entry in _SYSTEM_TABLES], counted=False)
    key = (ctx.email, ctx.role)
    now = time.monotonic()
    cached = _SYSTEM_CATALOG_CACHE.get(key)
    if cached and not refresh and now - cached[0] < _SYSTEM_CATALOG_TTL_SECONDS:
        return _system_catalog_document(cached[1], counted=True)
    tables = [_counted_system_row(entry, ctx) for entry in _SYSTEM_TABLES]
    _SYSTEM_CATALOG_CACHE[key] = (now, tables)
    for stale in [item for item, (at, _) in _SYSTEM_CATALOG_CACHE.items()
                  if now - at > _SYSTEM_CATALOG_TTL_SECONDS * 20]:
        _SYSTEM_CATALOG_CACHE.pop(stale, None)
    return _system_catalog_document(tables, counted=True)


def _system_catalog_document(tables: list, *, counted: bool) -> dict:
    read = {table["snapshotId"] for table in tables if table.get("snapshotId")}
    return {
        "success": True,
        "catalog": {"identifier": SYSTEM_CATALOG, "schema": SYSTEM_SCHEMA},
        "counted": counted,
        # One snapshot identity only when every family was read from the same
        # committed generation. A commit landing mid-read is reported, not
        # smoothed over into a single identity that was never true of all of it.
        "snapshot": {"id": next(iter(read)) if len(read) == 1 else None,
                     "consistent": len(read) <= 1},
        "tables": tables,
    }


def _system_row(entry: tuple[str, str]) -> dict:
    kind, table = entry
    return {"kind": kind, "table": table,
            "identifier": f"{SYSTEM_CATALOG}.{SYSTEM_SCHEMA}.{table}",
            "records": None, "truncated": False, "snapshotId": None, "error": None}


def _counted_system_row(entry: tuple[str, str], ctx: UserContext) -> dict:
    """One family's row, counted. Never raises: a family the Engine refuses is
    a row that says so, not a failed reading of the whole control plane."""
    row = _system_row(entry)
    try:
        counted = product_store.count_records(entry[0], ctx.email, ctx.role)
    except HTTPException as error:
        detail = error.detail
        row["error"] = detail if isinstance(detail, str) else str(
            detail.get("message") if isinstance(detail, dict) else detail)
        return row
    row.update(records=counted["records"], truncated=counted["truncated"],
               snapshotId=counted["snapshotId"])
    return row
