"""KaveonDB's own identity and store.

The platform's records — datasets, charts, dashboards, saved statements, chat
history, audit — are transactional rows in the Engine's built-in catalog. Two
facts about it appear in Studio, and they are different kinds of fact.

**The identifier is a protocol constant, not a setting.** The Engine's parser
recognises exactly the prefix `kaveon.product.` as the facade for its typed
product manifest — see `product_kind` in `engine/crates/sql/src/parser.rs` —
and refuses anything else. Making it configurable here would let a deployment
set a name that Studio displays, that `services.product_store` does not emit,
and that the Engine would reject. So it is pinned, named once, and mirrored by
every caller rather than retyped: `product_store` and `engine_system_store`
build their statements from `qualified()`, so there is one spelling of the
contract in the API and a drift is a test failure rather than a 400 in
production. "KaveonDB" is the product's name for the whole thing and is what
Studio says in prose; the identifier is for reading and for statements.

**Where it stores is a setting.** The deployment chooses that once —
`KAVEON_PRODUCT_STORAGE_MODE` and, for object storage, the account, container
and prefix — and Compose feeds the coordinator and the API from one root
`.env`, so a single value reaches both tiers and there is nothing to drift.
See docs/engineering/system-storage.md for why one configured store holds
everything Kaveon knows about itself.
"""
import os

# The facade the Engine's parser accepts. Pinned to
# engine/crates/sql/src/parser.rs::product_kind — changing it there is a
# breaking change to this contract and has to change here in the same commit.
CATALOG = "kaveon"
SCHEMA = "product"

_ADLS_MODES = frozenset({"adls", "abfss", "azure"})


def qualified(table: str) -> str:
    """One family's fully qualified name — what a statement targets and what
    Studio shows, so a reader copying a name out of the UI gets one that
    parses."""
    return f"{CATALOG}.{SCHEMA}.{table}"


def storage() -> dict:
    """Where the deployment has told KaveonDB to keep what it holds.

    Nothing here is invented: an unset variable is reported as unconfigured,
    because a location Studio cannot stand behind is worse than no location at
    all. `durable` says only whether the store survives the container that
    writes it — a host directory does not, which is right for a laptop and
    wrong for anything replaceable.
    """
    mode = os.getenv("KAVEON_PRODUCT_STORAGE_MODE", "").strip().lower()
    prefix = os.getenv("KAVEON_PRODUCT_ADLS_PREFIX", "").strip()
    if mode == "local":
        path = os.getenv("KAVEON_PRODUCT_LOCAL_PATH", "").strip()
        return {"configured": bool(path), "mode": "local", "durable": False,
                "account": None, "container": None, "prefix": None,
                "location": path or None}
    if mode in _ADLS_MODES:
        account = os.getenv("KAVEON_PRODUCT_ADLS_ACCOUNT", "").strip()
        container = os.getenv("KAVEON_PRODUCT_ADLS_CONTAINER", "").strip()
        configured = bool(account and container)
        return {"configured": configured, "mode": "adls", "durable": configured,
                "account": account or None, "container": container or None,
                "prefix": prefix or None,
                "location": f"{container}/{prefix}".rstrip("/") if configured else None}
    # Neither mode is set for this process. Say so rather than guessing one:
    # the API and the coordinator are configured together, so a missing
    # setting here means the deployment has not been told — not that it is
    # local. Reporting a laptop's default against an object store would be a
    # confident lie about the system of record.
    return {"configured": False, "mode": mode or None, "durable": None,
            "account": None, "container": None, "prefix": prefix or None,
            "location": None}
