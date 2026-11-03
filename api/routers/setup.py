"""Setup router — /api/v1/setup/status + /api/v1/admin/metadata.

KaveonDB is the product's system of record, so there is nothing for setup to
configure. What used to live here -- probing an external metadata database,
writing METADATA_* into the environment, creating its schema, resetting it --
described a system Kaveon no longer has. These two endpoints remain because
Studio reads them: one tells the frontend whether to show a setup overlay, the
other names the system holding product state on the connections page.
"""

from fastapi import APIRouter, Depends

from middleware.permissions import require_min_role
from services import system_catalog

router = APIRouter()


@router.get("/setup/status")
def setup_status():
    """Whether the application needs configuring. It never does: the authority
    is the Engine's catalog, which exists as soon as the Engine does."""
    return {"status": "ok", "authority": "kaveondb"}


@router.get("/admin/metadata")
def admin_get_metadata(ctx=Depends(require_min_role("Admin"))):
    """Which system holds product state (admin only).

    One system, not two: KaveonDB is both the query runtime and the system of
    record. `database` and `schema` are the product facade the Engine's parser
    accepts, named in `services.system_catalog` rather than retyped here. Host,
    port and endpoint are empty because the Engine is reached over the bridge,
    not a DSN. `storage` is where the deployment keeps it.
    """
    return {
        "db_type": "kaveondb",
        "label": "KaveonDB",
        "endpoint": "",
        "host": "",
        "port": "",
        "database": system_catalog.CATALOG,
        "schema": system_catalog.SCHEMA,
        "storage": system_catalog.storage(),
        "ui_configured": True,
        "authority": "kaveondb",
    }
