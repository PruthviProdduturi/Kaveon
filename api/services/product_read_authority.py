"""Explicit, family-scoped KaveonDB read-authority cutover."""

import os
from typing import Optional

from services import product_store


ENVIRONMENT_KEY = "KAVEONDB_READ_AUTHORITY_FAMILIES"
SUPPORTED_FAMILIES = frozenset({"datasets", "charts", "dashboards"})
_KIND = {"datasets": "dataset", "charts": "chart", "dashboards": "dashboard"}


class ProductReadAuthorityError(RuntimeError):
    """A cutover family could not be read safely from KaveonDB."""


def enabled(family: str) -> bool:
    """Return whether *family* has explicitly moved; reject unknown config."""
    if family not in SUPPORTED_FAMILIES:
        raise ProductReadAuthorityError(f"unsupported KaveonDB read-authority family: {family}")
    raw = os.getenv(ENVIRONMENT_KEY, "").strip()
    if not raw:
        return False
    configured = {item.strip().lower() for item in raw.split(",") if item.strip()}
    unknown = sorted(configured - SUPPORTED_FAMILIES)
    if unknown:
        raise ProductReadAuthorityError(
            "unknown KaveonDB read-authority families: " + ", ".join(unknown)
        )
    return family in configured


def read_document(
    family: str,
    record_id: str,
    actor: Optional[str],
    role: str,
) -> Optional[dict]:
    """Read one cutover record without any PostgreSQL fallback.

    KaveonDB is queried with the caller identity and an admin bridge role so the
    API can apply product visibility rules consistently with the legacy query.
    """
    if not enabled(family):
        raise ProductReadAuthorityError(f"{family} has not moved to KaveonDB read authority")
    principal = actor or "kaveon-system"
    target = product_store.read(_KIND[family], str(record_id), principal, "Admin")
    if target is None:
        return None
    document = target.get("document") if isinstance(target, dict) else None
    if not isinstance(document, dict):
        raise ProductReadAuthorityError(f"KaveonDB returned an invalid {family} document")
    visibility = document.get("visibility") or "internal"
    owner = document.get("created_by") or document.get("owner")
    permitted = (
        role == "Admin"
        or visibility == "published"
        or visibility == "internal" and role in {"Analyst", "Editor"}
        or visibility == "private" and bool(actor) and owner == actor
    )
    if actor is not None and not permitted:
        return None
    return dict(document)
