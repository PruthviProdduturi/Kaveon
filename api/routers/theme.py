"""Theme router — /api/v1/theme."""

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth
from models.theme import ThemeUpdate
import services.theme as svc

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}


@router.get("/theme")
def get_theme(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return svc.get_user_theme(user)


@router.put("/theme")
def save_theme(data: ThemeUpdate, user: str = Depends(require_auth)):
    svc.save_user_theme(user, data.theme_color)
    return {"success": True, "message": "Theme saved successfully", "theme_color": data.theme_color}


@router.delete("/theme")
def delete_theme(user: str = Depends(require_auth)):
    svc.delete_user_theme(user)
    return {"success": True, "message": "Theme reset to default"}
