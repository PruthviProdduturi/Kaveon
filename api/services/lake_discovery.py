"""Finds tables that are in the lake but not in the catalog.

Uploading a table publishes data; it does not make the product aware of it. The
freshness sweep recompiles the context of datasets that already exist, so a
newly published table is not stale — it is absent, and nothing was looking for
it. Registering by hand is the step everyone forgets, and the one that makes a
30 GB upload look like it did nothing.

**Scope is the safety property here.** Discovery only ever looks under the root
path of a catalog source that is registered and active. That is what keeps
benchmark scratch out of the product catalog: a ClickBench or TPC-H location
lives in a container nobody registered, so it is not somewhere this code can
reach. An earlier auto-registration swept such tables in, and the complaint was
fair.

**It refuses to guess a name.** A table's `location` is relative to its
catalog's root and the schema is a catalog-level concept, so the mapping from a
found directory to `catalog.schema.table` is only sometimes decidable:

  * two segments — `public/kaveon_events_enriched` — the first is the schema and
    the second the table, which is the convention every multi-schema catalog
    here already follows;
  * one segment — `daily_user_activity` — needs a schema, and is only
    unambiguous when the catalog has exactly one.

Anything else is reported rather than registered. A wrong guess is not a
cosmetic problem: it puts a table in the catalog under a name nobody chose, and
removing it later is a second deliberate act.
"""

from __future__ import annotations

import logging
from typing import Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)

# A table directory is one holding a Delta log. Parquet-only locations are not
# discovered: without a log there is no commit to establish what the table is,
# and every registered table in this installation is Delta.
DELTA_MARKER = "/_delta_log/"

# How many tables one run may register. A run that found a hundred new tables
# has almost certainly been pointed at the wrong prefix, and registering them
# would be a hundred catalog writes to undo by hand.
MAX_REGISTRATIONS = 20


def ingestible_sources(actor: str) -> List[dict]:
    """Active catalog sources whose storage is a lake prefix we can list."""
    from services import product_store

    out = []
    for record in product_store.list_records("source", actor, "Admin", max_records=1000):
        document = record.get("document") or {}
        if not document.get("is_active") or document.get("lifecycle") != "active":
            continue
        storage = document.get("storage_config") or {}
        account, container = storage.get("account"), storage.get("container")
        root = (storage.get("root_path") or "").strip("/")
        # KaveonDB is the product catalog itself and carries a `base_path`
        # rather than a lake prefix; it holds no discoverable user tables.
        if not account or not container or not root:
            continue
        name = document.get("catalog_identity") or document.get("name")
        if not name:
            continue
        out.append({"catalog": name, "account": account, "container": container, "root": root})
    return out


def table_directories(client, root: str, *, max_objects: int = 10000) -> Optional[List[str]]:
    """Relative paths under `root` that hold a Delta log.

    `None` when the prefix holds more objects than one listing may return. That
    is reported rather than raised: a root too large to scan is a fact about
    the installation, and it must not take a scheduled sweep down with it.
    """
    try:
        objects = client.list(root, max_objects=max_objects)
    except RuntimeError as error:
        logger.warning("could not list %s: %s", root, error)
        return None
    prefix = root.rstrip("/") + "/"
    found = set()
    for entry in objects:
        path = entry.get("path") or ""
        marker = path.find(DELTA_MARKER)
        if marker < 0 or not path.startswith(prefix):
            continue
        # The table is the directory holding the log, taken relative to root.
        found.add(path[len(prefix):marker + 1].strip("/"))
    return sorted(part for part in found if part)


def registered_locations(catalog: str, actor: str) -> Optional[set]:
    """The relative locations the catalog already holds, or `None` if it could
    not be read — which must not be mistaken for "it holds nothing", because
    that would register every table in the lake a second time."""
    from services import engine_bridge

    try:
        schemas = engine_bridge.schemas(catalog, actor, "Admin").get("schemas") or []
    except Exception as error:  # noqa: BLE001 - an unreadable catalog is skipped
        logger.warning("could not read schemas of %s: %s", catalog, error)
        return None
    locations = set()
    for schema in schemas:
        name = schema.get("name") if isinstance(schema, dict) else schema
        try:
            listed = engine_bridge.tables(catalog, name, actor, "Admin")
        except Exception as error:  # noqa: BLE001
            logger.warning("could not read tables of %s.%s: %s", catalog, name, error)
            return None
        rows = listed.get("tables", listed) if isinstance(listed, dict) else listed
        for table in rows or []:
            table_name = table.get("name") if isinstance(table, dict) else table
            definition = engine_bridge.table_definition_by_id(
                f"table:{catalog}:{name}:{table_name}", actor, "Admin")
            if isinstance(definition, dict):
                location = (definition.get("location") or "").strip("/")
                if location:
                    locations.add(location)
    return locations


def placement(relative: str, schemas: List[str]) -> Tuple[Optional[str], Optional[str], str]:
    """Where a found directory belongs: `(schema, table, reason)`.

    `schema` is None when the mapping is not decidable, and the reason says
    why, so a run reports what it declined instead of leaving a silent gap.
    """
    parts = [part for part in relative.split("/") if part]
    if len(parts) == 2:
        return parts[0], parts[1], "schema from the path"
    if len(parts) == 1:
        if len(schemas) == 1:
            return schemas[0], parts[0], f"the catalog's only schema, {schemas[0]}"
        return None, None, (
            f"a one-segment location needs a schema and this catalog has "
            f"{len(schemas)}: {', '.join(sorted(schemas))}")
    return None, None, f"a {len(parts)}-segment location has no unambiguous schema"


def discover(actor: str, *, register: bool = False,
             limit: int = MAX_REGISTRATIONS) -> Dict[str, list]:
    """Report, and optionally register, tables present in the lake but absent
    from the catalog.

    `register` is off by default: the reading is useful on its own, and a sweep
    that writes to the catalog should have been asked to.
    """
    from services import engine_bridge
    from services.adls_artifact_client import AzureArtifactClient

    report: Dict[str, list] = {"registered": [], "undecidable": [], "unreadable": [], "found": []}
    for source in ingestible_sources(actor):
        catalog = source["catalog"]
        try:
            client = AzureArtifactClient(source["account"], source["container"])
        except ValueError as error:
            report["unreadable"].append({"catalog": catalog, "reason": str(error)})
            continue
        directories = table_directories(client, source["root"])
        if directories is None:
            report["unreadable"].append(
                {"catalog": catalog, "reason": f"{source['root']} could not be listed"})
            continue
        known = registered_locations(catalog, actor)
        if known is None:
            report["unreadable"].append(
                {"catalog": catalog, "reason": "the catalog's own tables could not be read"})
            continue
        schemas = [schema.get("name") if isinstance(schema, dict) else schema
                   for schema in (engine_bridge.schemas(catalog, actor, "Admin").get("schemas") or [])]

        for relative in directories:
            if relative in known:
                continue
            report["found"].append({"catalog": catalog, "location": relative})
            schema, table, reason = placement(relative, [s for s in schemas if s])
            if not schema or not table:
                report["undecidable"].append(
                    {"catalog": catalog, "location": relative, "reason": reason})
                continue
            if not register:
                continue
            if len(report["registered"]) >= limit:
                report["undecidable"].append({
                    "catalog": catalog, "location": relative,
                    "reason": f"this run's registration bound of {limit} was reached"})
                continue
            try:
                created = engine_bridge.create_table_inferred(
                    catalog, schema, table, relative, "Delta", actor, "Admin")
            except Exception as error:  # noqa: BLE001 - one table, not the run
                report["undecidable"].append({
                    "catalog": catalog, "location": relative,
                    "reason": f"registration failed: {type(error).__name__}: {str(error)[:200]}"})
                continue
            if not isinstance(created, dict) or not created.get("ok", True):
                report["undecidable"].append({
                    "catalog": catalog, "location": relative,
                    "reason": f"registration refused: {str(created)[:200]}"})
                continue
            report["registered"].append(
                {"catalog": catalog, "schema": schema, "table": table,
                 "location": relative, "why": reason})
    return report
