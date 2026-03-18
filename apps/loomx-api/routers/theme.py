"""Theme router — /api/v1/theme."""

from fastapi import APIRouter, Request, Response, HTTPException
from middleware.auth import get_user_email
import services.theme as svc

router = APIRouter()


def _require_email(request: Request) -> str:
    email = get_user_email(request)
    if email == "anonymous":
        raise HTTPException(status_code=401, detail="User email not found in request headers")
    return email


@router.get("/theme")
def get_theme(request: Request, response: Response):
    response.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
    response.headers["Pragma"] = "no-cache"
    response.headers["Expires"] = "0"
    user_email = _require_email(request)
    return svc.get_user_theme(user_email)


@router.put("/theme")
def save_theme(data: dict, request: Request):
    user_email = _require_email(request)
    theme_color = data.get("theme_color")
    if not theme_color:
        raise HTTPException(status_code=400, detail="theme_color is required")
    svc.save_user_theme(user_email, theme_color)
    return {"success": True, "message": "Theme saved successfully", "theme_color": theme_color}


@router.delete("/theme")
def delete_theme(request: Request):
    user_email = _require_email(request)
    svc.delete_user_theme(user_email)
    return {"success": True, "message": "Theme reset to default"}
