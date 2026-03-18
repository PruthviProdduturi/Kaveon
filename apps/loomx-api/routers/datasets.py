"""Datasets router — /api/v1/datasets."""

from fastapi import APIRouter, Request, Response, HTTPException, Depends
from middleware.auth import require_auth
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
def list_datasets(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return svc.list_datasets()


@router.get("/datasets/summary")
def datasets_summary(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    items = svc.list_datasets(user)
    return {"count": len(items), "recent": items}


@router.get("/datasets/{dataset_id}")
def get_dataset(dataset_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    dataset = svc.get_dataset_by_id(dataset_id, user)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return dataset


@router.post("/datasets", status_code=201)
def create_dataset(data: DatasetCreate, user: str = Depends(require_auth)):
    return svc.create_dataset(data.model_dump(exclude_none=True), user)


@router.put("/datasets/{dataset_id}")
def update_dataset(dataset_id: str, data: DatasetUpdate, user: str = Depends(require_auth)):
    result = svc.update_dataset(dataset_id, data.model_dump(exclude_none=True), user)
    if not result:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return result


@router.patch("/datasets/{dataset_id}")
def patch_dataset(dataset_id: str, data: DatasetUpdate, user: str = Depends(require_auth)):
    result = svc.update_dataset(dataset_id, data.model_dump(exclude_none=True), user)
    if not result:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return result


@router.delete("/datasets/{dataset_id}", status_code=204)
def delete_dataset(dataset_id: str, user: str = Depends(require_auth)):
    deleted = svc.delete_dataset(dataset_id)
    if not deleted:
        raise HTTPException(status_code=404, detail="Dataset not found")


@router.put("/datasets/{dataset_id}/favorite")
def toggle_dataset_favorite(dataset_id: str, user: str = Depends(require_auth)):
    dataset = svc.get_dataset_by_id(dataset_id)
    if not dataset:
        raise HTTPException(status_code=404, detail="Dataset not found")
    return fav_svc.toggle_favorite(
        {"object_type": "dataset", "object_id": dataset_id, "object_name": dataset.get("name")},
        user,
    )
