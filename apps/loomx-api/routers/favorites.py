"""Favorites router — /api/v1/favorites."""

from fastapi import APIRouter, Request, Response, HTTPException
from middleware.auth import get_user_email
import services.favorites as svc

router = APIRouter()


@router.get("/favorites")
def list_favorites(request: Request, response: Response):
    response.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
    response.headers["Pragma"] = "no-cache"
    response.headers["Expires"] = "0"
    user_id = get_user_email(request)
    return svc.list_favorites(user_id)


@router.post("/favorites", status_code=201)
def create_favorite(data: dict, request: Request):
    if not data.get("object_type") or not data.get("object_id") or not data.get("object_name"):
        raise HTTPException(status_code=400, detail="object_type, object_id, and object_name are required")
    user_id = get_user_email(request)
    return svc.create_favorite(data, user_id)


@router.post("/favorites/toggle")
def toggle_favorite(data: dict, request: Request):
    if not data.get("object_type") or not data.get("object_id") or not data.get("object_name"):
        raise HTTPException(status_code=400, detail="object_type, object_id, and object_name are required")
    user_id = get_user_email(request)
    return svc.toggle_favorite(data, user_id)


@router.delete("/favorites/{fav_id}", status_code=204)
def delete_favorite(fav_id: str, request: Request):
    user_id = get_user_email(request)
    deleted = svc.delete_favorite_by_id(fav_id, user_id)
    if not deleted:
        raise HTTPException(status_code=404, detail="Favorite not found")
