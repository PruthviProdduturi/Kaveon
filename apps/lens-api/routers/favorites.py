"""Favorites router — /api/v1/favorites."""

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth
from models.favorites import FavoriteCreate
import services.favorites as svc

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}


@router.get("/favorites")
def list_favorites(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return svc.list_favorites(user)


@router.post("/favorites", status_code=201)
def create_favorite(data: FavoriteCreate, user: str = Depends(require_auth)):
    return svc.create_favorite(data.model_dump(), user)


@router.post("/favorites/toggle")
def toggle_favorite(data: FavoriteCreate, user: str = Depends(require_auth)):
    return svc.toggle_favorite(data.model_dump(), user)


@router.delete("/favorites/{fav_id}", status_code=204)
def delete_favorite(fav_id: str, user: str = Depends(require_auth)):
    deleted = svc.delete_favorite_by_id(fav_id, user)
    if not deleted:
        raise HTTPException(status_code=404, detail="Favorite not found")
