"""Datasets router — /api/v1/datasets."""

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_read, can_write, can_publish
from middleware.demo import allowed_in_demo
from models.datasets import DatasetCreate, DatasetUpdate
import services.datasets as svc
import services.favorites as fav_svc
from services import engine_datasets

router = APIRouter()
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
    return svc.create_dataset(payload, ctx.email)


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
