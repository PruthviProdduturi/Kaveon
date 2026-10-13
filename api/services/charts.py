"""Charts service — port of charts.service.ts."""

import json
import logging
import os
import threading
import time
import uuid
from datetime import datetime, timezone
from typing import List, Optional
import database.metadata as db
from services import chart_backfill, product_outbox, product_shadow_read, product_store, thumbnails

logger = logging.getLogger(__name__)

VALID_VISIBILITY = {"private", "internal", "published"}
_SCHEMA_CACHE_TTL_SECONDS = 60.0
# How stale a dataset-name index a single chart read will accept before reading
# a current one. A list read never accepts a stale index.
_DATASET_NAME_MAX_AGE_SECONDS = 30.0
_schema_cache: tuple[float, str] | None = None
_schema_lock = threading.Lock()


def _outbox_enabled() -> bool:
    return os.getenv("KAVEON_CHART_OUTBOX_ENABLED") == "true"


def _outbox_row(transaction, chart_id: str, layout: str) -> dict:
    if layout == "modern":
        row = transaction.query_one("""SELECT id,name,description,dataset_id,chart_type,config,visibility,
            created_by,modified_by,created_at,modified_at FROM charts WHERE id=@param0""", [chart_id])
    else:
        row = transaction.query_one("""SELECT id,name,description,chart_type,query_config,viz_config,visibility,
            created_by,updated_by,created_at,updated_at FROM charts WHERE id=@param0""", [int(chart_id)])
    if not row:
        raise RuntimeError("Chart disappeared before outbox capture")
    preliminary = chart_backfill._document(row, layout, 1)
    dataset = product_store.read("dataset", preliminary["dataset_id"], preliminary["created_by"], "Admin")
    revision = dataset.get("revision") if dataset else None
    if type(revision) is not int or revision < 1:
        raise RuntimeError("KaveonDB dataset revision is unavailable for chart outbox capture")
    return chart_backfill._document(row, layout, revision)


def _enqueue(transaction, operation: str, chart_id: str, actor: str, layout: str) -> None:
    document = {} if operation == "delete" else _outbox_row(transaction, chart_id, layout)
    owner = str(document.get("created_by") or actor)
    product_outbox.enqueue(transaction, family="charts", operation=operation, record_id=str(chart_id),
                           payload=document, actor=actor, owner=owner)


def _chart_schema() -> str:
    """Return one known charts layout from a bounded, read-only capability probe."""
    global _schema_cache
    now = time.monotonic()
    with _schema_lock:
        if _schema_cache and now - _schema_cache[0] < _SCHEMA_CACHE_TTL_SECONDS:
            return _schema_cache[1]
        rows = db.query(
            "SELECT column_name FROM information_schema.columns WHERE table_name = @param0",
            ["charts"],
        )["rows"]
        columns = {str(row.get("column_name", "")).casefold() for row in rows}
        modern = {"id", "dataset_id", "config", "modified_at", "modified_by"}
        legacy = {"id", "query_config", "viz_config", "updated_at", "updated_by"}
        if modern <= columns:
            layout = "modern"
        elif legacy <= columns:
            layout = "legacy"
        else:
            raise RuntimeError("Charts metadata schema is not a supported layout")
        _schema_cache = (now, layout)
        return layout


def _configs(row: dict) -> tuple[dict, dict]:
    """Read the PostgreSQL `config` envelope and tolerate legacy flat config."""
    stored = {}
    try:
        stored = json.loads(row.get("config") or "{}")
    except Exception:
        pass
    if not isinstance(stored, dict):
        stored = {}
    query_config = stored.get("query_config")
    viz_config = stored.get("viz_config")
    # Earlier installations stored the query config directly in `config`.
    if not isinstance(query_config, dict):
        query_config = stored
    if not isinstance(viz_config, dict):
        viz_config = {}
    return query_config, viz_config


def _adapt(row: dict, layout: str) -> dict:
    if layout == "legacy":
        try:
            query_config = json.loads(row.get("query_config") or "{}")
            viz_config = json.loads(row.get("viz_config") or "{}")
        except Exception:
            query_config, viz_config = {}, {}
    else:
        query_config, viz_config = _configs(row)
    config = {**query_config, **viz_config}

    dataset_id = str(row.get("dataset_id") or query_config.get("dataset_id") or "")

    return {
        "id": str(row["id"]),
        "name": row.get("name"),
        "description": row.get("description"),
        "dataset_id": dataset_id,
        "dataset_name": row.get("dataset_name"),
        "chart_type": row.get("chart_type") or "table",
        "thumbnail": row.get("thumbnail"),
        "has_thumbnail": bool(row.get("thumbnail")),
        # The metadata tables carry no capture time, so a preview read from one
        # reports none and the refresh pass treats it as current rather than
        # guessing. Only a KaveonDB product record stamps it.
        "thumbnail_updated_at": None,
        "config": config,
        "query_config": query_config,
        "viz_config": viz_config,
        "sql_text": None,
        "visibility": row.get("visibility") or "internal",
        "created_at": row.get("created_at"),
        "updated_at": row.get("updated_at"),
        "created_by": row.get("created_by"),
        "owner": row.get("created_by"),
        "modified_by": (row.get("updated_by") if layout == "legacy" else row.get("modified_by")) or row.get("created_by"),
        "favorite": row.get("favorite") == 1,
    }


def _dataset_names(documents: list[dict], actor: str | None,
                   *, max_age_seconds: float = 0.0) -> dict[str, str]:
    """Resolve the dataset names a batch of chart records needs in one read.

    A chart record names its dataset by identity only, and a card shows the
    dataset's name, so the names are resolved for the whole batch in a single
    bounded index read rather than one read per chart. A point read reuses a
    recent index instead of paying a round trip of its own; the list path reads
    a current one alongside the charts themselves.
    """
    from services import datasets
    if not any(document.get("dataset_id") for document in documents):
        return {}
    return datasets.dataset_name_index(actor or "kaveon-system",
                                       max_age_seconds=max_age_seconds)


def _speculative_dataset_names(actor: str | None):
    """The dataset-name index, or the exception raised reading it.

    Read alongside the chart listing rather than after it, because the listing
    is the only thing that decides whether the names are wanted and waiting for
    that answer costs a round trip of its own. The failure is carried back
    instead of raised so that a listing which turns out to need no names — no
    chart in the deployment names a dataset — is still answered, exactly as it
    was when the index was read on demand.
    """
    from services import datasets
    try:
        return datasets.dataset_name_index(actor or "kaveon-system")
    except Exception as error:  # noqa: BLE001 - re-raised by a caller that needs it
        return error


def _adapt_product(document: dict, dataset_names: dict[str, str]) -> dict:
    query_config = document.get("query_config") or {}
    viz_config = document.get("viz_config") or {}
    if not isinstance(query_config, dict) or not isinstance(viz_config, dict):
        raise RuntimeError("KaveonDB chart configuration is invalid")
    return {
        **document, "config": {**query_config, **viz_config}, "sql_text": None,
        "dataset_name": dataset_names.get(str(document.get("dataset_id") or "")),
        "thumbnail": document.get("thumbnail"),
        "has_thumbnail": bool(document.get("thumbnail")),
        "thumbnail_updated_at": document.get("thumbnail_updated_at"),
        "owner": document.get("created_by"), "favorite": bool(document.get("favorite", False)),
    }


def _product_document(data: dict, chart_id: str, actor: str, prior: dict | None = None) -> dict:
    prior = prior or {}
    query_config = data.get("query_config", data.get("config", prior.get("query_config", {}))) or {}
    viz_config = data.get("viz_config", prior.get("viz_config", {})) or {}
    if not isinstance(query_config, dict) or not isinstance(viz_config, dict):
        raise ValueError("Chart configuration must be an object")
    dataset_id = str(data.get("dataset_id", prior.get("dataset_id")) or "")
    if not dataset_id:
        raise ValueError("Chart dataset_id is required")
    query_config = {**query_config, "dataset_id": data.get("dataset_id", dataset_id)}
    dataset = product_store.read("dataset", dataset_id, actor, "Admin")
    dataset_revision = dataset.get("revision") if dataset else None
    if type(dataset_revision) is not int or dataset_revision < 1:
        raise ValueError(f"Chart dataset {dataset_id} is unavailable")
    visibility = data.get("visibility", prior.get("visibility", "internal")) or "internal"
    if visibility not in VALID_VISIBILITY:
        raise ValueError("Chart visibility is invalid")
    now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    owner = str(prior.get("created_by") or actor)
    chart_type = data.get("chart_type", prior.get("chart_type", "table")) or "table"
    if not isinstance(chart_type, str) or not chart_type:
        raise ValueError("Chart type is invalid")
    # The Library preview is part of the record, so it has to survive an update
    # that does not mention it. Absent rather than null when there is none, so a
    # chart that was never rendered carries no key at all.
    thumbnail = thumbnails.normalise(
        data["thumbnail"] if "thumbnail" in data else prior.get("thumbnail"),
        thumbnails.CHART_MAX_CHARS,
    )
    # When the stored image was taken. The Library's refresh pass needs to tell
    # a current preview from one the chart has since outgrown, and `updated_at`
    # alone cannot say that. Stamped only when the image itself changes, so an
    # edit that leaves the preview alone makes the preview look older than the
    # record — which is exactly what makes it outstanding. A record whose
    # preview predates this field carries no stamp and is read as "cannot
    # tell", never as stale.
    captured_at = prior.get("thumbnail_updated_at")
    if not isinstance(captured_at, str) or not captured_at:
        captured_at = None
    if thumbnail and thumbnail != prior.get("thumbnail"):
        captured_at = now
    # Storing a preview is not an edit of the chart. It used to be written as
    # one, which left `updated_at` ahead of the capture time the preview was
    # stored with, so every stored preview immediately looked stale, and a
    # chart jumped to the top of the Library's recently-modified order merely
    # by being rendered. A write that carries nothing but the preview keeps the
    # record's own modification identity.
    preview_only = bool(prior) and bool(data) and set(data) <= {"thumbnail"}
    prior_updated_at = prior.get("updated_at")
    updated_at = prior_updated_at if preview_only and isinstance(prior_updated_at, str) and prior_updated_at else now
    prior_modified_by = prior.get("modified_by")
    modified_by = prior_modified_by if preview_only and isinstance(prior_modified_by, str) and prior_modified_by else actor
    return {
        **({"thumbnail": thumbnail} if thumbnail else {}),
        **({"thumbnail_updated_at": captured_at} if thumbnail and captured_at else {}),
        "id": chart_id, "name": data.get("name", prior.get("name")),
        "description": data.get("description", prior.get("description")),
        "dataset_id": dataset_id, "dataset_revision": dataset_revision,
        "chart_type": chart_type, "query_config": query_config,
        "viz_config": viz_config, "visibility": visibility,
        "created_at": prior.get("created_at") or now, "updated_at": updated_at,
        "created_by": owner, "modified_by": modified_by,
    }


def _summary(chart: dict) -> dict:
    """A list entry: everything except the preview image itself.

    Seventy inline data URIs would make every Library load megabytes of JSON the
    browser cannot cache, so the list says only whether a preview exists and the
    card fetches each one from /charts/{id}/thumbnail.
    """
    return {**chart, "thumbnail": None}


def get_chart_thumbnail(chart_id: str, user_email: str, role: str) -> Optional[str]:
    """Return one chart's stored preview data URI, or None when it has none.

    A cover needs the chart's bytes and the authority to serve them, and
    nothing else. Reading it as a whole Library card also asked whether the
    viewer had favorited the chart and what its dataset is called — two
    independent product reads that cannot change the image — which turned a
    single round trip into three. Seventy cards paid that per card.
    """
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        document = product_read_authority.read_document(
            "charts", chart_id, user_email, role, with_favorite=False,
        )
        return (document or {}).get("thumbnail") or None
    chart = get_chart_by_id(chart_id, user_email, role, include_thumbnail=True)
    return (chart or {}).get("thumbnail") or None


def _vis_clause(role_idx: int, email_idx: int, alias: str = "c") -> str:
    return (
        f"({alias}.visibility = 'published' "
        f"OR ({alias}.visibility = 'internal' AND @param{role_idx} IN ('Analyst', 'Editor', 'Admin')) "
        f"OR ({alias}.visibility = 'private' AND {alias}.created_by = @param{email_idx}) "
        f"OR @param{role_idx} = 'Admin')"
    )


def list_charts(user_email: str, role: str = "Viewer") -> List[dict]:
    from services import product_read_authority, product_reads
    if product_read_authority.enabled("charts"):
        # The chart listing and the dataset-name index are independent, and the
        # index is what turns a chart's dataset identity into the name on its
        # card. Reading it after the charts came back added its own half second
        # to the tab, and the only thing the listing decided was whether the
        # index was wanted at all.
        #
        # Only worth overlapping while the names come from the product store:
        # a deployment whose datasets are still in PostgreSQL resolves them
        # with a local query rather than a round trip, and must not be made to
        # run that query speculatively for a chart list that needs no names.
        if not product_read_authority.enabled("datasets"):
            documents = product_read_authority.list_documents("charts", user_email, role)
            names = _dataset_names(documents, user_email)
            return [_summary(_adapt_product(item, names)) for item in documents]
        documents, names = product_reads.gather(
            lambda: product_read_authority.list_documents("charts", user_email, role),
            lambda: _speculative_dataset_names(user_email),
        )
        if isinstance(names, BaseException):
            if any(document.get("dataset_id") for document in documents):
                raise names
            names = {}
        return [_summary(_adapt_product(item, names)) for item in documents]
    layout = _chart_schema()
    if layout == "legacy":
        return [_summary(chart) for chart in _legacy_list_charts(user_email, role)]
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT c.id, c.name, c.description, c.dataset_id, c.chart_type, c.config,
               c.created_by, c.modified_by, c.created_at, c.modified_at,
               c.visibility, c.thumbnail, ds.dataset_name,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(c.id AS NVARCHAR(255))
            AND f.object_type = 'chart' AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds ON ds.id = c.dataset_id
        WHERE c.id IS NOT NULL AND {vis}
        ORDER BY c.modified_at DESC
    """, [user_email, role])
    return [_summary(_adapt(r, layout)) for r in result["rows"]]


def get_chart_by_id(
    chart_id: str,
    user_email: Optional[str] = None,
    role: str = "Admin",
    *,
    include_thumbnail: bool = False,
) -> Optional[dict]:
    """
    role defaults to 'Admin' for internal service calls so chart rendering
    is never blocked by visibility. Pass the actual role from user-facing endpoints.

    The preview image is withheld unless asked for: it is a binary resource the
    browser should fetch and cache on its own, not a field every caller of this
    function has to carry. get_chart_thumbnail is the one reader that wants it.
    """
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        document = product_read_authority.read_document(
            "charts", chart_id, user_email, role,
        )
        if document is not None:
            document.setdefault("favorite", False)
            names = _dataset_names([document], user_email,
                                   max_age_seconds=_DATASET_NAME_MAX_AGE_SECONDS)
            chart = _adapt_product(document, names)
            return chart if include_thumbnail else _summary(chart)
        return None
    layout = _chart_schema()
    if layout == "legacy":
        return _legacy_get_chart(chart_id, user_email, role)
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT c.id, c.name, c.description, c.dataset_id, c.chart_type, c.config,
                   c.created_by, c.modified_by, c.created_at, c.modified_at,
                   c.visibility, c.thumbnail
            FROM dbo.charts c
            WHERE c.id = @param0 AND c.id IS NOT NULL AND {vis}
        """, [chart_id, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, description, dataset_id, chart_type, config,
                   created_by, modified_by, created_at, modified_at, visibility, thumbnail
            FROM dbo.charts WHERE id = @param0 AND id IS NOT NULL
        """, [chart_id])
    chart = _adapt(row, layout) if row else None
    if chart is not None and user_email:
        try:
            report = product_shadow_read.compare_chart(chart, user_email, role)
            if report.get("enabled"):
                logger.info("chart_shadow_read %s", json.dumps(report, sort_keys=True))
        except Exception as error:
            logger.warning("chart_shadow_read_error type=%s", type(error).__name__)
    if chart is None or include_thumbnail:
        return chart
    return _summary(chart)


def create_chart(data: dict, user_id: str) -> dict:
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        chart_id = str(uuid.uuid4())
        document = _product_document(data, chart_id, user_id)
        product_store.transact([
            product_store.ProductMutation("create", "chart", chart_id, document)
        ], user_id, "Analyst")
        created = get_chart_by_id(chart_id, user_id, "Admin")
        if not created:
            raise RuntimeError("KaveonDB did not return the created chart")
        return created
    if _chart_schema() == "legacy":
        return _legacy_create_chart(data, user_id)
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    query_config = data.get("query_config") or data.get("config") or {}
    if "dataset_id" in data and data["dataset_id"] is not None:
        query_config = {**query_config, "dataset_id": data["dataset_id"]}

    visibility = data.get("visibility") or "internal"
    if visibility not in VALID_VISIBILITY:
        visibility = "internal"

    chart_id = str(uuid.uuid4())
    envelope = {"query_config": query_config, "viz_config": data.get("viz_config") or {}}
    statement = """
        INSERT INTO charts (id, name, description, dataset_id, chart_type, config,
                           visibility, created_by, modified_by, created_at, modified_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8, @param9, @param10)
    """
    params = [
        chart_id, data["name"], data.get("description"), data["dataset_id"], data["chart_type"],
        json.dumps(envelope), visibility, user_id, user_id, now, now,
    ]
    if _outbox_enabled():
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            _enqueue(transaction, "create", chart_id, user_id, "modern")
    else:
        db.execute(statement, params)
    created = get_chart_by_id(chart_id)
    if not created:
        raise RuntimeError("Failed to retrieve created chart")
    return created


def update_chart(chart_id: str, data: dict, actor: str | None = None,
                 role: str = "Analyst") -> Optional[dict]:
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        if not actor:
            raise RuntimeError("KaveonDB chart update requires actor identity")
        current = product_store.read("chart", chart_id, actor, "Admin")
        if current is None:
            return None
        revision, document = current.get("revision"), current.get("document")
        if type(revision) is not int or revision < 1 or not isinstance(document, dict):
            raise RuntimeError("KaveonDB returned invalid chart revision state")
        updated = _product_document(data, chart_id, actor, document)
        product_store.transact([
            product_store.ProductMutation("update", "chart", chart_id, updated, revision)
        ], product_store.writer(document.get("created_by"), actor, role), "Analyst")
        return get_chart_by_id(chart_id, actor, "Admin")
    if _chart_schema() == "legacy":
        return _legacy_update_chart(chart_id, data, actor)
    if not get_chart_by_id(chart_id):
        return None
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updates, params, i = [], [], 0

    for field_name, col in [("name", "name"), ("description", "description"), ("chart_type", "chart_type")]:
        if field_name in data:
            updates.append(f"{col} = @param{i}"); params.append(data[field_name]); i += 1
    if "dataset_id" in data:
        updates.append(f"dataset_id = @param{i}"); params.append(data["dataset_id"]); i += 1
    if "query_config" in data or "viz_config" in data or "config" in data:
        existing = get_chart_by_id(chart_id) or {}
        query_config = data.get("query_config", data.get("config", existing.get("query_config") or {}))
        viz_config = data.get("viz_config", existing.get("viz_config") or {})
        updates.append(f"config = @param{i}")
        params.append(json.dumps({"query_config": query_config or {}, "viz_config": viz_config or {}})); i += 1
    if "visibility" in data:
        vis = data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"
        updates.append(f"visibility = @param{i}"); params.append(vis); i += 1
    if "thumbnail" in data:
        updates.append(f"thumbnail = @param{i}")
        params.append(thumbnails.normalise(data["thumbnail"], thumbnails.CHART_MAX_CHARS)); i += 1

    updates.append(f"modified_at = @param{i}"); params.append(now); i += 1
    params.append(chart_id)

    statement = f"UPDATE charts SET {', '.join(updates)} WHERE id = @param{i}"
    if _outbox_enabled():
        if not actor:
            raise RuntimeError("Chart outbox capture requires actor identity")
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            _enqueue(transaction, "update", chart_id, actor, "modern")
    else:
        db.execute(statement, params)
    return get_chart_by_id(chart_id)


def delete_chart(chart_id: str, actor: str | None = None,
                 role: str = "Analyst") -> bool:
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        if not actor:
            raise RuntimeError("KaveonDB chart delete requires actor identity")
        current = product_store.read("chart", chart_id, actor, "Admin")
        if current is None:
            return False
        revision, document = current.get("revision"), current.get("document")
        if type(revision) is not int or revision < 1 or not isinstance(document, dict):
            raise RuntimeError("KaveonDB returned invalid chart revision state")
        product_store.transact([
            product_store.ProductMutation("delete", "chart", chart_id,
                                          expected_revision=revision)
        ], product_store.writer(document.get("created_by"), actor, role), "Analyst")
        return True
    if _chart_schema() == "legacy":
        if not _outbox_enabled():
            return db.execute("DELETE FROM charts WHERE id = @param0", [int(chart_id)]) > 0
        if not actor:
            raise RuntimeError("Chart outbox capture requires actor identity")
        with db.transaction() as transaction:
            row = transaction.query_one("SELECT created_by FROM charts WHERE id=@param0 FOR UPDATE", [int(chart_id)])
            if not row:
                return False
            deleted = transaction.execute("DELETE FROM charts WHERE id = @param0", [int(chart_id)]) > 0
            product_outbox.enqueue(transaction, family="charts", operation="delete", record_id=str(chart_id),
                                   payload={}, actor=actor, owner=str(row["created_by"]))
            return deleted
    if not _outbox_enabled():
        return db.execute("DELETE FROM charts WHERE id = @param0", [chart_id]) > 0
    if not actor:
        raise RuntimeError("Chart outbox capture requires actor identity")
    with db.transaction() as transaction:
        row = transaction.query_one("SELECT created_by FROM charts WHERE id=@param0 FOR UPDATE", [chart_id])
        if not row:
            return False
        deleted = transaction.execute("DELETE FROM charts WHERE id = @param0", [chart_id]) > 0
        product_outbox.enqueue(transaction, family="charts", operation="delete", record_id=chart_id,
                               payload={}, actor=actor, owner=str(row["created_by"]))
        return deleted


def count_charts() -> int:
    from services import product_read_authority
    if product_read_authority.enabled("charts"):
        return len(product_read_authority.list_documents("charts", "kaveon-system", "Admin"))
    result = db.query_one("SELECT COUNT(*) as count FROM charts")
    return result.get("count") or 0


def _legacy_list_charts(user_email: str, role: str) -> List[dict]:
    vis = _vis_clause(1, 0)
    result = db.query(f"""
        SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
               c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at,
               c.visibility, c.thumbnail, ds.dataset_name,
               CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f ON f.object_id = CAST(c.id AS NVARCHAR(255))
            AND f.object_type = 'chart' AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds ON ds.id = TRY_CAST(JSON_VALUE(c.query_config, '$.dataset_id') AS INT)
        WHERE c.id IS NOT NULL AND {vis}
        ORDER BY c.updated_at DESC
    """, [user_email, role])
    return [_adapt(row, "legacy") for row in result["rows"]]


def _legacy_get_chart(chart_id: str, user_email: Optional[str], role: str) -> Optional[dict]:
    chart_id_int = int(chart_id)
    if user_email:
        vis = _vis_clause(2, 1)
        row = db.query_one(f"""
            SELECT c.id, c.name, c.description, c.chart_type, c.query_config, c.viz_config,
                   c.created_on, c.created_by, c.changed_on, c.updated_by, c.created_at, c.updated_at,
                   c.visibility, c.thumbnail
            FROM dbo.charts c WHERE c.id = @param0 AND c.id IS NOT NULL AND {vis}
        """, [chart_id_int, user_email, role])
    else:
        row = db.query_one("""
            SELECT id, name, description, chart_type, query_config, viz_config,
                   created_on, created_by, changed_on, updated_by, created_at, updated_at,
                   visibility, thumbnail
            FROM dbo.charts WHERE id = @param0 AND id IS NOT NULL
        """, [chart_id_int])
    return _adapt(row, "legacy") if row else None


def _legacy_create_chart(data: dict, user_id: str) -> dict:
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    query_config = data.get("query_config") or data.get("config") or {}
    if data.get("dataset_id") is not None:
        query_config = {**query_config, "dataset_id": data["dataset_id"]}
    visibility = data.get("visibility") if data.get("visibility") in VALID_VISIBILITY else "internal"
    statement = """
        INSERT INTO charts (name, description, chart_type, query_config, viz_config, visibility,
                            created_on, created_by, changed_on, updated_by, created_at, updated_at)
        VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7, @param8, @param9, @param10, @param11)
    """
    params = [data["name"], data.get("description"), data["chart_type"], json.dumps(query_config),
              json.dumps(data.get("viz_config") or {}), visibility, now, user_id, now, user_id, now, now]
    lookup = "SELECT TOP 1 id FROM charts WHERE name = @param0 AND created_by = @param1 ORDER BY id DESC"
    if _outbox_enabled():
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            inserted = transaction.query_one(lookup, [data["name"], user_id])
            if not inserted:
                raise RuntimeError("Failed to retrieve created chart")
            _enqueue(transaction, "create", str(inserted["id"]), user_id, "legacy")
    else:
        db.execute(statement, params)
        inserted = db.query_one(lookup, [data["name"], user_id])
    if not inserted:
        raise RuntimeError("Failed to retrieve created chart")
    return _legacy_get_chart(str(inserted["id"]), None, "Admin")


def _legacy_update_chart(chart_id: str, data: dict, actor: str | None = None) -> Optional[dict]:
    if not _legacy_get_chart(chart_id, None, "Admin"):
        return None
    now, updates, params, index = datetime.now(timezone.utc).replace(tzinfo=None), [], [], 0
    for field, column in [("name", "name"), ("description", "description"), ("chart_type", "chart_type")]:
        if field in data:
            updates.append(f"{column} = @param{index}"); params.append(data[field]); index += 1
    for field in ("query_config", "viz_config"):
        if field in data:
            updates.append(f"{field} = @param{index}"); params.append(json.dumps(data[field])); index += 1
    if "visibility" in data:
        updates.append(f"visibility = @param{index}"); params.append(data["visibility"] if data["visibility"] in VALID_VISIBILITY else "internal"); index += 1
    if "thumbnail" in data:
        updates.append(f"thumbnail = @param{index}")
        params.append(thumbnails.normalise(data["thumbnail"], thumbnails.CHART_MAX_CHARS)); index += 1
    updates.append(f"changed_on = @param{index}"); params.append(now); index += 1
    updates.append(f"updated_at = @param{index}"); params.append(now); index += 1
    params.append(int(chart_id))
    statement = f"UPDATE charts SET {', '.join(updates)} WHERE id = @param{index}"
    if _outbox_enabled():
        if not actor:
            raise RuntimeError("Chart outbox capture requires actor identity")
        with db.transaction() as transaction:
            transaction.execute(statement, params)
            _enqueue(transaction, "update", chart_id, actor, "legacy")
    else:
        db.execute(statement, params)
    return _legacy_get_chart(chart_id, None, "Admin")
