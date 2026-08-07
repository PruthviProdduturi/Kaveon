"""User recents router — /api/v1/user/recents."""

from fastapi import APIRouter, Depends
from pydantic import BaseModel
from middleware.auth import require_user_context, UserContext
import services.user_recents as svc

router = APIRouter()


class RecentItem(BaseModel):
    item_id: str
    label: str
    href: str
    type: str  # "dashboard" | "chart" | "dataset" | "query" | "chat"


@router.get("/user/recents")
def get_recents(ctx: UserContext = Depends(require_user_context)):
    return svc.get_recents(ctx.email)


@router.post("/user/recents")
def add_recent(item: RecentItem, ctx: UserContext = Depends(require_user_context)):
    svc.add_recent(ctx.email, item.item_id, item.label, item.href, item.type)
    return {"status": "ok"}


@router.delete("/user/recents/{item_id}")
def remove_recent(item_id: str, ctx: UserContext = Depends(require_user_context)):
    svc.remove_recent(ctx.email, item_id)
    return {"status": "ok"}
