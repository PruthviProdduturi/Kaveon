"""Catalog access through the platform bridge — /api/v1/engine/admin/catalog-access.

Who may see, query and change which Engine catalog. The grants live in the
Engine's KaveonDB transaction authority (the `catalog_grants` family) and the
Engine enforces them at every path; this router is the administrator's door
to them. It forwards the verified Kaveon principal and role to the Engine,
which applies its own admin gate, and hands revision conflicts back as 409
so the Studio can reload and retry. Nothing in a request body names the
actor: the Engine records the authenticated administrator.
"""
from typing import Any

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from services import engine_bridge

router = APIRouter(tags=["engine-catalog-access"])

ACCESS_LEVELS = ("browse", "query", "manage")


class GrantBody(BaseModel):
    principal: str = Field(min_length=1, max_length=96)
    catalog: str = Field(min_length=1, max_length=64)
    access: str
    revision: int | None = Field(default=None, ge=1)


class RevokeBody(BaseModel):
    principal: str = Field(min_length=1, max_length=96)
    catalog: str = Field(min_length=1, max_length=64)
    revision: int = Field(ge=1)


class ImportBody(BaseModel):
    source: str = "open"
    apply: bool = False


def _document(value: Any, key: str) -> dict:
    if not isinstance(value, dict) or key not in value:
        raise HTTPException(502, "Engine returned an invalid catalog access document")
    return value


def _clean(body: GrantBody | RevokeBody) -> None:
    for field in ("principal", "catalog"):
        value = getattr(body, field)
        if value != value.strip() or any(ch.isspace() for ch in value):
            raise HTTPException(422, f"The {field} must not contain whitespace")


@router.get("/engine/admin/catalog-access")
def catalog_access(ctx: UserContext = Depends(require_min_role("Admin"))):
    return _document(engine_bridge.catalog_access(ctx.email, ctx.role), "grants")


@router.put("/engine/admin/catalog-access/grants")
def grant_catalog_access(body: GrantBody, ctx: UserContext = Depends(require_min_role("Admin"))):
    if body.access not in ACCESS_LEVELS:
        raise HTTPException(422, "Access must be one of browse, query or manage")
    _clean(body)
    payload = {"principal": body.principal, "catalog": body.catalog, "access": body.access}
    if body.revision is not None:
        payload["revision"] = body.revision
    return _document(engine_bridge.grant_catalog_access(payload, ctx.email, ctx.role), "grant")


@router.delete("/engine/admin/catalog-access/grants")
def revoke_catalog_access(body: RevokeBody, ctx: UserContext = Depends(require_min_role("Admin"))):
    _clean(body)
    payload = {"principal": body.principal, "catalog": body.catalog, "revision": body.revision}
    return _document(engine_bridge.revoke_catalog_access(payload, ctx.email, ctx.role), "revoked")


@router.get("/engine/admin/catalog-access/effective/{principal}")
def effective_catalog_access(principal: str, ctx: UserContext = Depends(require_min_role("Admin"))):
    if not principal.strip() or any(ch.isspace() for ch in principal):
        raise HTTPException(422, "A principal is required")
    return _document(engine_bridge.effective_catalog_access(principal, ctx.email, ctx.role), "grants")


@router.post("/engine/admin/catalog-access/import")
def import_catalog_access(body: ImportBody, ctx: UserContext = Depends(require_min_role("Admin"))):
    """The reconciliation of a deployment that ran open: the Engine proposes
    a grant per principal its audit ledger has seen, per catalog, at the
    role's ceiling; nothing is recorded unless `apply` is set."""
    if body.source != "open":
        raise HTTPException(422, "The only import source is 'open'")
    return _document(engine_bridge.import_catalog_access({"source": "open", "apply": body.apply}, ctx.email, ctx.role), "proposed")


@router.get("/engine/catalog-access/me")
def my_catalog_access(ctx: UserContext = Depends(require_min_role("Viewer"))):
    """The caller's own standing: the catalogs they may reach and at what level."""
    return _document(engine_bridge.my_catalog_access(ctx.email, ctx.role), "catalogs")
