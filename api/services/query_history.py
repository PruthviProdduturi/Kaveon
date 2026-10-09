"""Query history service — persists SQL Lab / dataset query runs.

Columns match the live `query_history` table exactly. T-SQL idioms
(TOP / OUTPUT INSERTED) are translated per-dialect by database.metadata.
"""

import uuid
import json
import threading
import time
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db
from services import product_outbox, product_store, product_read_authority
from services.query_history_backfill import document as migration_document
import os
import logging
MAX_DELETE_FANOUT=100
MAX_HISTORY_PER_OWNER=1_000


_BASE_COLS = (
    "id, sql_text, database_name, executed_at, execution_time, row_count, "
    "status, error_message, user_email, trigger_source, dataset_id, tables_used"
)
_ENGINE_COLS = ", engine_query_id, engine_details"
_SCHEMA_CACHE_TTL_SECONDS = 60.0
_schema_cache: tuple[float, bool] | None = None
_schema_lock = threading.Lock()


def _supports_engine_details() -> bool:
    """Capability-detect additive telemetry columns for rolling upgrades."""
    global _schema_cache
    now = time.monotonic()
    with _schema_lock:
        if _schema_cache and now - _schema_cache[0] < _SCHEMA_CACHE_TTL_SECONDS:
            return _schema_cache[1]
        try:
            rows = db.query(
                "SELECT column_name FROM information_schema.columns WHERE table_name = @param0",
                ["query_history"],
            )["rows"]
        except Exception:
            _schema_cache = (now, False)
            return False
        columns = {str(row.get("column_name", "")).casefold() for row in rows}
        supported = {"engine_query_id", "engine_details"} <= columns
        _schema_cache = (now, supported)
        return supported


def _engine_metadata(data: dict) -> tuple[Optional[str], Optional[str]]:
    details = data.get("engine_details")
    if not isinstance(details, dict):
        return data.get("engine_query_id"), None
    safe = {
        key: details.get(key)
        for key in (
            "state", "error", "rows_are_preview", "scan_metrics_complete", "submitted_at_ms",
            "completed_at_ms", "timings", "scans", "stages", "context",
        )
        if key in details
    }
    if isinstance(safe.get("state"), str):
        safe["state"] = safe["state"][:32]
    raw_error = safe.get("error")
    if isinstance(raw_error, str):
        safe["error"] = raw_error[:2048]
    elif isinstance(raw_error, dict):
        safe["error"] = {
            key: str(raw_error[key])[:2048]
            for key in ("code", "message") if key in raw_error
        }
    elif raw_error is not None:
        safe.pop("error")
    return data.get("engine_query_id") or details.get("id"), json.dumps(safe, separators=(",", ":"))


def list_history(user_id: Optional[str], limit: int = 50) -> List[dict]:
    from services import product_read_authority
    if product_read_authority.enabled("query_history"):
        actor = user_id if user_id and user_id != "all" else "kaveon-system"
        role = "Admin" if not user_id or user_id == "all" else "Viewer"
        return product_read_authority.list_documents("query_history", actor, role)[:limit]
    columns = _BASE_COLS + (_ENGINE_COLS if _supports_engine_details() else "")
    fetch_all = not user_id or user_id == "all"
    if fetch_all:
        result = db.query(
            f"SELECT TOP (@param0) {columns} FROM query_history ORDER BY executed_at DESC",
            [limit],
        )
    else:
        result = db.query(
            f"SELECT TOP (@param1) {columns} FROM query_history "
            f"WHERE user_email = @param0 ORDER BY executed_at DESC",
            [user_id, limit],
        )
    for row in result["rows"]:
        raw = row.get("engine_details")
        if isinstance(raw, str):
            try:
                row["engine_details"] = json.loads(raw)
            except (TypeError, ValueError):
                row["engine_details"] = None
    if not fetch_all:
        try:
            from services import product_shadow_read
            report=product_shadow_read.observe_query_history_list(result["rows"],user_id)
            if report.get("enabled"):logging.getLogger(__name__).info("query_history_shadow %s",report)
        except Exception as error:logging.getLogger(__name__).warning("query_history_shadow_error type=%s",type(error).__name__)
    return result["rows"]


# How many records each owner holds, as last observed. Retention is a bound,
# not an invariant, so an approximate count is the right instrument: the worst
# a stale one does is list a little early or let an owner sit a few records
# over the bound until the next write corrects it. A process that has never
# listed for an owner assumes the bound is in reach and lists once.
_HISTORY_COUNTS: dict[str, int] = {}
_HISTORY_COUNT_LOCK = threading.Lock()
# Far enough below the bound that the writes arriving while a sweep is still
# running cannot carry an owner meaningfully past it.
_RETENTION_CHECK_MARGIN = 50
# Records removed per sweep. A transaction carries at most a hundred
# mutations, and the sweep's transaction holds nothing else, so it takes the
# whole allowance — which leaves the owner far enough below the bound that the
# writes that follow schedule nothing.
_RETENTION_TRIM_BATCH = MAX_DELETE_FANOUT

# Owners with a sweep already in flight. Dashboards write a dozen rows at once
# and every one of them would otherwise see the same over-bound count and
# start its own sweep, which would then race for the same records' revisions.
_RETENTION_SWEEPS: set[str] = set()


def _at_retention_bound(owner: str) -> bool:
    with _HISTORY_COUNT_LOCK:
        known = _HISTORY_COUNTS.get(owner)
    return known is None or known >= MAX_HISTORY_PER_OWNER - _RETENTION_CHECK_MARGIN


def _remember_history_count(owner: str, count: int) -> None:
    with _HISTORY_COUNT_LOCK:
        _HISTORY_COUNTS[owner] = count


def _count_one_more(owner: str) -> None:
    """One record added. An owner never counted stays uncounted, so the sweep
    that establishes the count is the one that sets it."""
    with _HISTORY_COUNT_LOCK:
        known = _HISTORY_COUNTS.get(owner)
        if known is not None:
            _HISTORY_COUNTS[owner] = known + 1


def _claim_retention_sweep(owner: str) -> bool:
    with _HISTORY_COUNT_LOCK:
        if owner in _RETENTION_SWEEPS:
            return False
        _RETENTION_SWEEPS.add(owner)
        return True


def _release_retention_sweep(owner: str) -> None:
    with _HISTORY_COUNT_LOCK:
        _RETENTION_SWEEPS.discard(owner)


def run_retention_sweep(owner: str) -> int:
    """Bring *owner* back under the retention bound, and report what it took.

    Listing an owner's history is paged a hundred records at a time, so at the
    bound it is ten round trips, and the trim that follows is a further hundred
    statements inside one transaction. None of that belongs in front of a
    query's response: the bound is a bound, so an owner sitting a few records
    over it until this finishes is correct, while a reader waiting eight
    seconds for it is not.
    """
    records = product_store.list_records(
        "query_history", owner, "Viewer", max_records=MAX_HISTORY_PER_OWNER)
    _remember_history_count(owner, len(records))
    over = len(records) - MAX_HISTORY_PER_OWNER
    if over < 0:
        return 0

    def ordering(record):
        item = record.get("document") or {}
        return (str(item.get("executed_at") or ""), str(item.get("id") or ""))

    # Enough to clear the overage plus the headroom that keeps the writes which
    # follow from scheduling another sweep, and never more than one transaction
    # carries.
    take = min(over + _RETENTION_TRIM_BATCH, MAX_DELETE_FANOUT)
    mutations = []
    for oldest in sorted(records, key=ordering)[:take]:
        revision = oldest.get("revision")
        item = oldest.get("document")
        if type(revision) is not int or revision < 1 or not isinstance(item, dict) \
                or item.get("user_email") != owner:
            raise RuntimeError("KaveonDB query history retention state is invalid")
        mutations.append(product_store.ProductMutation(
            "delete", "query_history", str(item["id"]), expected_revision=revision))
    if not mutations:
        return 0
    product_store.transact(mutations, owner, "Analyst")
    _remember_history_count(owner, len(records) - len(mutations))
    return len(mutations)


def _sweep_retention_quietly(owner: str) -> None:
    try:
        run_retention_sweep(owner)
    except Exception as error:
        # The owner stays over the bound and the next write schedules another
        # sweep. A failure here must never reach the query that triggered it,
        # which has already been answered.
        logging.getLogger(__name__).warning(
            "query_history_retention_sweep_failed type=%s", type(error).__name__)
    finally:
        _release_retention_sweep(owner)


def _schedule_retention(owner: str) -> None:
    """Start a sweep for *owner* if one is warranted and none is running."""
    if not _at_retention_bound(owner) or not _claim_retention_sweep(owner):
        return
    try:
        threading.Thread(target=_sweep_retention_quietly, args=(owner,),
                         name="query-history-retention", daemon=True).start()
    except RuntimeError:
        # Threads unavailable during interpreter shutdown: do it here rather
        # than leave the claim held and retention never running again.
        _sweep_retention_quietly(owner)


def create_history(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    started_at = data.get("started_at")
    if isinstance(started_at, (int, float)):
        started_at = datetime.fromtimestamp(started_at / 1000, tz=timezone.utc).replace(tzinfo=None)
    elif started_at is None:
        started_at = now

    execution_time = data.get("duration_ms") or 0
    trigger_source = data.get("trigger_source") or "lab"
    # id is a varchar with no DB default (like dashboards) — generate app-side.
    new_id = str(uuid.uuid4())

    engine_query_id, engine_details = _engine_metadata(data)
    if product_read_authority.enabled("query_history"):
        result = {
            "id": new_id, "sql_text": data["sql_text"],
            "database_name": data.get("database_name"), "executed_at": started_at,
            "execution_time": execution_time, "row_count": data.get("row_count"),
            "status": data["status"], "error_message": data.get("error_message"),
            "user_email": user_id, "trigger_source": trigger_source,
            "dataset_id": data.get("dataset_id"), "tables_used": data.get("tables_used"),
        }
        document = migration_document(result)
        # The write is one mutation and nothing else. Retention used to ride
        # along in this transaction: at the bound it listed the owner's whole
        # history — ten paged reads — and then committed a hundred deletes as a
        # hundred sequential statements, so one write in thirty cost about
        # eight seconds while the Engine behind it had answered in ~200ms.
        # It runs behind the response now instead.
        product_store.transact([product_store.ProductMutation(
            "create", "query_history", new_id, document)], user_id, "Analyst")
        _count_one_more(user_id)
        _schedule_retention(user_id)
        return {**result, "engine_query_id": engine_query_id,
                "engine_details": json.loads(engine_details) if engine_details else None}
    supports_details=_supports_engine_details()
    sql = """
        INSERT INTO query_history (
            id, sql_text, database_name, executed_at, execution_time, row_count,
            status, error_message, user_email, trigger_source, dataset_id, tables_used,
            engine_query_id, engine_details
        ) VALUES (
            @param0, @param1, @param2, @param3, @param4, @param5,
            @param6, @param7, @param8, @param9, @param10, @param11, @param12, @param13
        )
    """
    params = [
        new_id, data["sql_text"], data.get("database_name"), started_at,
        execution_time, data.get("row_count"), data["status"], data.get("error_message"),
        user_id, trigger_source, data.get("dataset_id"), data.get("tables_used"),
        engine_query_id, engine_details,
    ]
    if not supports_details:
        sql = """
        INSERT INTO query_history (
            id, sql_text, database_name, executed_at, execution_time, row_count,
            status, error_message, user_email, trigger_source, dataset_id, tables_used
        ) VALUES (
            @param0, @param1, @param2, @param3, @param4, @param5,
            @param6, @param7, @param8, @param9, @param10, @param11
        )
        """;params = [
        new_id,
        data["sql_text"],
        data.get("database_name"),
        started_at,
        execution_time,
        data.get("row_count"),
        data["status"],
        data.get("error_message"),
        user_id,
        trigger_source,
        data.get("dataset_id"),
        data.get("tables_used"),
        ]

    result = {
        "id": new_id, "sql_text": data["sql_text"], "database_name": data.get("database_name"),
        "executed_at": started_at, "execution_time": execution_time, "row_count": data.get("row_count"),
        "status": data["status"], "error_message": data.get("error_message"), "user_email": user_id,
        "trigger_source": trigger_source, "dataset_id": data.get("dataset_id"), "tables_used": data.get("tables_used"),
    }
    if os.getenv("KAVEON_QUERY_HISTORY_OUTBOX_ENABLED")=="true":
        with db.transaction() as transaction:
            transaction.execute("SELECT pg_advisory_xact_lock(hashtext(@param0))",[user_id])
            transaction.execute(sql,params)
            product_outbox.enqueue(transaction,family="query_history",operation="create",record_id=new_id,payload=migration_document(result),actor=user_id,owner=user_id)
            evicted=transaction.query("SELECT id FROM query_history WHERE user_email=@param0 ORDER BY executed_at DESC,id DESC OFFSET @param1 LIMIT 2 FOR UPDATE",[user_id,MAX_HISTORY_PER_OWNER])["rows"]
            if len(evicted)>1:raise RuntimeError("query history retention requires bounded cleanup")
            if evicted:
                transaction.execute("DELETE FROM query_history WHERE id=@param0",[evicted[0]["id"]])
                product_outbox.enqueue(transaction,family="query_history",operation="delete",record_id=str(evicted[0]["id"]),payload={},actor=user_id,owner=user_id)
    else: db.query(sql,params)

    return {
        "id": new_id,
        "sql_text": data["sql_text"],
        "database_name": data.get("database_name"),
        "status": data["status"],
        "trigger_source": trigger_source,
        "executed_at": started_at,
        "user_email": user_id,
        "execution_time": execution_time,
        "row_count": data.get("row_count"),
        "engine_query_id": engine_query_id,
        "engine_details": json.loads(engine_details) if engine_details else None,
    }


def delete_all_history(user_id: str) -> int:
    if product_read_authority.enabled("query_history"):
        records = product_store.list_records(
            "query_history", user_id, "Viewer", max_records=MAX_HISTORY_PER_OWNER)
        if len(records) > MAX_DELETE_FANOUT:
            raise RuntimeError("query history delete exceeds its fanout bound")
        mutations = []
        for record in records:
            document, revision = record.get("document"), record.get("revision")
            if not isinstance(document, dict) or document.get("user_email") != user_id \
                    or type(revision) is not int or revision < 1:
                raise RuntimeError("KaveonDB query history delete state is invalid")
            mutations.append(product_store.ProductMutation(
                "delete", "query_history", str(document["id"]), expected_revision=revision))
        if mutations:
            product_store.transact(mutations, user_id, "Analyst")
        return len(mutations)
    if os.getenv("KAVEON_QUERY_HISTORY_OUTBOX_ENABLED")=="true":
        with db.transaction() as transaction:
            rows=transaction.query("SELECT id FROM query_history WHERE user_email=@param0 ORDER BY id LIMIT @param1 FOR UPDATE",[user_id,MAX_DELETE_FANOUT+1])["rows"]
            if len(rows)>MAX_DELETE_FANOUT:raise RuntimeError("query history delete exceeds its fanout bound")
            for row in rows:
                transaction.execute("DELETE FROM query_history WHERE id=@param0",[row["id"]]);product_outbox.enqueue(transaction,family="query_history",operation="delete",record_id=str(row["id"]),payload={},actor=user_id,owner=user_id)
            return len(rows)
    return db.execute(
        "DELETE FROM query_history WHERE user_email = @param0",
        [user_id],
    )
