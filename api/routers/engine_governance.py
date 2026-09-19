"""Engine governance through the platform bridge: resource groups and the
audit ledger, administrators only.

The browser never holds an Engine credential. Each call carries the verified
Kaveon principal and role to the Engine, which applies its own admin gate;
the platform role model decides who may reach these routes at all.
"""
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import StreamingResponse

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from services import engine_bridge

router = APIRouter(tags=["engine-governance"])

AUDIT_QUERY_KEYS = ("since", "until", "principal", "kind", "query_id", "limit", "cursor")


def _document(value: Any) -> dict:
    if not isinstance(value, dict) or not isinstance(value.get("groups"), list):
        raise HTTPException(502, "Engine returned an invalid resource-group document")
    return value


@router.get("/engine/admin/resource-groups")
def resource_groups(ctx: UserContext = Depends(require_min_role("Admin"))):
    return _document(engine_bridge.resource_groups(ctx.email, ctx.role))


@router.put("/engine/admin/resource-groups")
def replace_resource_groups(payload: dict, ctx: UserContext = Depends(require_min_role("Admin"))):
    if not isinstance(payload.get("groups"), list) or not isinstance(payload.get("selectors", []), list):
        raise HTTPException(422, "A resource-group document needs a 'groups' list and an optional 'selectors' list")
    return _document(engine_bridge.replace_resource_groups(payload, ctx.email, ctx.role))


@router.get("/engine/audit")
def audit(request: Request, ctx: UserContext = Depends(require_min_role("Admin"))):
    params = {key: value for key, value in request.query_params.items() if key in AUDIT_QUERY_KEYS and value != ""}
    if request.query_params.get("format") == "jsonl":
        stream = engine_bridge.audit_export(params, ctx.email, ctx.role)
        return StreamingResponse(
            stream, media_type="application/x-ndjson",
            headers={"Content-Disposition": 'attachment; filename="kaveon-audit.jsonl"'},
        )
    result = engine_bridge.audit(params, ctx.email, ctx.role)
    if not isinstance(result, dict) or not isinstance(result.get("records"), list):
        raise HTTPException(502, "Engine returned an invalid audit page")
    return result
