"""Datasets router — /api/v1/datasets."""

import logging

from fastapi import APIRouter, BackgroundTasks, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_read, can_write, can_publish
from middleware.demo import allowed_in_demo
from models.datasets import DatasetCreate, DatasetUpdate
import services.datasets as svc
import services.favorites as fav_svc
import services.query_history as query_history_svc
import services.user_recents as recents_svc
from routers import reference_guard
from services import engine_datasets

router = APIRouter()
log = logging.getLogger(__name__)


def _compile_dlm(dataset_id: str, actor: str, role: str) -> None:
    """Compile a new dataset's DLM artifact, out of the request's way.

    A dataset with no artifact answers nothing on the DLM path, which is the
    product's primary path: until something compiles it, the context banner
    reads "No context compiled yet" and every question falls through. Nothing
    was compiling it. `create_dataset` returned the row and stopped, and the
    only caller of the generate endpoint was the bulk repair action in
    Settings, so a dataset created the ordinary way stayed mute until someone
    noticed and pressed a button.

    Failure is logged, never raised. The dataset exists and is valid either
    way, and a background task that raises would turn a successful create into
    a 500 the client has already been told did not happen. A dataset whose
    compile failed is recoverable -- the generate endpoint is idempotent and
    still there -- so the honest outcome is a created dataset and a logged
    reason, not a lost one.
    """
    try:
        from dlm import engine as dlm_engine

        result = dlm_engine.generate_dlm(dataset_id, actor=actor, role=role)
        if not result.get("ok"):
            log.warning(
                "dlm_autocompile_declined dataset=%s reason=%s",
                dataset_id, result.get("reason"),
            )
    except Exception as error:  # noqa: BLE001 - the create must still stand
        log.warning(
            "dlm_autocompile_failed dataset=%s type=%s",
            dataset_id, type(error).__name__,
        )


NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


@router.get("/datasets")
def list_datasets(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    return svc.list_datasets(ctx.email, ctx.role)


@router.get("/datasets/summary")
def datasets_summary(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    items = svc.list_datasets(ctx.email, ctx.role)
    return {"count": len(items), "recent": items}


@router.get("/datasets/schemas")
def datasets_schemas(response: Response, ctx: UserContext = Depends(require_user_context)):
    """Every visible dataset's askable shape in one read.

    Declared ahead of ``/datasets/{dataset_id}`` so "schemas" is not taken for
    a dataset identifier.
    """
    response.headers.update(NO_CACHE)
    items = svc.list_dataset_schemas(ctx.email, ctx.role)
    return {"count": len(items), "schemas": items}


@router.get("/datasets/{dataset_id}")
def get_dataset(dataset_id: str, response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    dataset = svc.get_dataset_by_id(dataset_id, ctx.email, ctx.role)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    # Which SQL a client may write against this dataset. The server resolves it
    # from the registered catalog sources, because only the server knows
    # whether `database_name` is an Engine catalog or a registered external
    # database. Studio used to infer it from the schema name, with `public` and
    # `climate_energy` written in as the two that meant "Engine" — so every
    # dataset in any other schema was handed SQL Server syntax the Engine
    # rejects.
    dataset["sql_dialect"] = _dialect_for(dataset.get("database_name") or "")
    return dataset


def _dialect_for(database: str) -> str:
    """`engine` for a catalog the Engine serves, `tsql` otherwise.

    An Engine catalog takes double-quoted identifiers and `LIMIT`; Fabric SQL
    and Azure SQL take bracketed identifiers and `TOP`. Unknown resolves to
    `engine`: every dataset in a deployment without a registered external
    database reads the lake, and guessing T-SQL there is the failure this
    replaces.
    """
    from routers.sql import _is_engine_catalog
    if not database:
        return "engine"
    try:
        return "engine" if _is_engine_catalog(database) else "tsql"
    except Exception:
        # A catalog listing that cannot be read is not a reason to answer with
        # a dialect the Engine refuses.
        return "engine"


@router.get("/datasets/{dataset_id}/columns")
def get_columns(dataset_id: str, response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    dataset = svc.get_dataset_by_id(dataset_id, ctx.email, ctx.role)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return dataset.get("columns") or []


@router.post("/datasets", status_code=201)
def create_dataset(
    data: DatasetCreate,
    ctx: UserContext = Depends(require_min_role("Analyst")),
    # Last, and optional, so `create_dataset(body, ctx)` keeps working: this
    # route is called directly by tests and internal callers, and taking the
    # second position would have silently bound a UserContext to it. FastAPI
    # injects by annotation rather than position, so the served path is
    # unaffected; `lab.py` carries the same shape for the same reason.
    background_tasks: BackgroundTasks = None,
):
    # A create has no prior value for any field, so "omitted" and "sent as
    # null" mean the same thing here — the field has no value — and dropping
    # the null is the correct reading. It is also the safe one: the store
    # defaults an absent `schema_name` to `dbo` and an absent `database_name`
    # to the empty string, both NOT NULL columns that an explicit null would
    # violate. Only an update has two meanings to keep apart.
    payload = data.model_dump(exclude_none=True)
    # Only Editors+ may publish directly on create
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"
    # An Engine-backed dataset takes its names, columns and semantics from
    # the Engine's table definition; the caller names the table by its id.
    payload = engine_datasets.apply_binding(payload, ctx.email, ctx.role)
    created = svc.create_dataset(payload, ctx.email)
    # Compiled after the response, not during it: profiling a dataset reads
    # its source, and a create that blocked on that would be slow in exactly
    # the case the DLM matters most -- a large table. The client gets its 201
    # immediately and the artifact appears behind it.
    dataset_id = created.get("id") if isinstance(created, dict) else None
    if dataset_id and background_tasks is not None:
        background_tasks.add_task(_compile_dlm, str(dataset_id), ctx.email, ctx.role)
    return created


def _apply_dataset_update(dataset_id: str, data: DatasetUpdate, ctx: UserContext) -> dict:
    """Apply a partial dataset update.

    `exclude_unset` is what makes the update honest: the payload carries only
    the fields the caller actually sent, so an omitted field is left as it
    stands while a field sent as `null` reaches the store as `None` and is
    cleared. `exclude_none` could not tell those two apart and answered 200
    to a clear request that changed nothing. `DatasetUpdate` refuses `null`
    for the fields that have no cleared state, so every `None` that arrives
    here names a field the store can genuinely empty."""
    existing = svc.get_dataset_by_id(dataset_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dataset not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this dataset")

    payload = data.model_dump(exclude_unset=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"
    payload = engine_datasets.apply_binding(payload, ctx.email, ctx.role, existing=existing)

    result = svc.update_dataset(dataset_id, payload, ctx.email)
    if not result:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return result


@router.put("/datasets/{dataset_id}")
def update_dataset(
    dataset_id: str,
    data: DatasetUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    return _apply_dataset_update(dataset_id, data, ctx)


@router.patch("/datasets/{dataset_id}")
def patch_dataset(
    dataset_id: str,
    data: DatasetUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    return _apply_dataset_update(dataset_id, data, ctx)


@router.delete("/datasets/{dataset_id}", status_code=204)
def delete_dataset(dataset_id: str, ctx: UserContext = Depends(require_user_context)):
    existing = svc.get_dataset_by_id(dataset_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dataset not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to delete this dataset")
    # A chart names its dataset as a reference in the product catalog, and the
    # catalog refuses to delete a record another record still points at. So a
    # dataset with charts over it was answered with the Engine's own refusal —
    # a 502 quoting `product record 'chart/<id>' references missing
    # 'dataset/<id>'` — where the legacy PostgreSQL path had always answered
    # 409 and said which charts to deal with first. Same verdict here, and the
    # dependents are counted at system scope so a chart the caller cannot see
    # blocks the delete instead of becoming that 502 one line later.
    total, names = reference_guard.dependents(
        "charts", lambda document: str(document.get("dataset_id")) == str(dataset_id),
        actor=ctx.email, role=ctx.role)
    if total:
        raise HTTPException(status_code=409, detail=reference_guard.message(
            total, names, "chart", "Delete those charts before deleting this dataset."))
    # References first, then the record — as charts and dashboards do. A
    # recents entry and a favorite each NAME this dataset, they belong to
    # whoever opened or favorited it rather than to the caller, and nobody
    # else can clear them: so the delete clears them. A failure to clear is
    # raised rather than swallowed, because proceeding would only produce the
    # catalog's rejection one line later.
    #
    # A query-history row names it too, and that one is easy to miss because
    # nobody chose it: the Engine derives a dataset reference from the row's
    # own `dataset_id`, so a dataset ANYONE had ever run a single query
    # against was undeletable, with no favorite or recent in sight. Cleared on
    # the same footing, bounded and per owner.
    try:
        recents_svc.remove_recent_all_users(f"dataset-{dataset_id}", "dataset")
        fav_svc.remove_favorite_all_users("dataset", dataset_id)
        query_history_svc.remove_dataset_all_users(dataset_id)
    except Exception as error:
        raise HTTPException(
            status_code=502,
            detail=f"Could not clear this dataset's references, so it was not deleted: {error}",
        ) from error
    svc.delete_dataset(dataset_id, ctx.email)


@router.put("/datasets/{dataset_id}/favorite")
@allowed_in_demo
def toggle_dataset_favorite(dataset_id: str, user: str = Depends(require_auth)):
    dataset = svc.get_dataset_by_id(dataset_id)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return fav_svc.toggle_favorite(
        {"object_type": "dataset", "object_id": dataset_id, "object_name": dataset.get("name")},
        user,
    )
