"""Datasets router — /api/v1/datasets."""

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_read, can_write, can_publish
from models.datasets import DatasetCreate, DatasetUpdate
import services.datasets as svc
import services.favorites as fav_svc

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


@router.get("/datasets/{dataset_id}")
def get_dataset(dataset_id: str, response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    dataset = svc.get_dataset_by_id(dataset_id, ctx.email, ctx.role)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return dataset


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
    payload = data.model_dump(exclude_none=True)
    # Only Editors+ may publish directly on create
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"
    return svc.create_dataset(payload, ctx.email)


@router.put("/datasets/{dataset_id}")
def update_dataset(
    dataset_id: str,
    data: DatasetUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    existing = svc.get_dataset_by_id(dataset_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dataset not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this dataset")

    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"

    result = svc.update_dataset(dataset_id, payload, ctx.email)
    if not result:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return result


@router.patch("/datasets/{dataset_id}")
def patch_dataset(
    dataset_id: str,
    data: DatasetUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    existing = svc.get_dataset_by_id(dataset_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dataset not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this dataset")

    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"

    result = svc.update_dataset(dataset_id, payload, ctx.email)
    if not result:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return result


@router.delete("/datasets/{dataset_id}", status_code=204)
def delete_dataset(dataset_id: str, ctx: UserContext = Depends(require_user_context)):
    existing = svc.get_dataset_by_id(dataset_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dataset not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to delete this dataset")
    svc.delete_dataset(dataset_id)


@router.put("/datasets/{dataset_id}/favorite")
def toggle_dataset_favorite(dataset_id: str, user: str = Depends(require_auth)):
    dataset = svc.get_dataset_by_id(dataset_id)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return fav_svc.toggle_favorite(
        {"object_type": "dataset", "object_id": dataset_id, "object_name": dataset.get("name")},
        user,
    )
