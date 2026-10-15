"""What still references a record the caller is trying to delete.

The product catalog validates typed references when a transaction commits, and
refuses to delete a record another record still points at:

    invalid transaction: product record 'dashboard/<id>'
    references missing 'chart/<id>'

That rejection reaches the API as an Engine refusal and would be reported to
the user as a 502 naming a catalog record key — which tells them nothing they
can act on. The reference graph is small and known (api/routers/charts.py and
api/routers/dashboards.py purge the per-user bookkeeping; the Engine derives
the rest in `engine/crates/server/src/transaction_api.rs`), so a delete can
ask first and say what is in the way.

Two kinds of reference need two different answers:

  * Per-user bookkeeping — a recents entry, a favorite — belongs to whoever
    opened or favorited the record rather than to the person deleting it.
    Nobody else can clear it, so the delete clears it (a bounded cross-owner
    fanout; see services.favorites.remove_favorite_all_users).

  * Content — a chart on a dashboard, a chart over a dataset — belongs to its
    own author and deleting it on their behalf is not this request's business.
    So the delete is refused with the dependents named, which is an action the
    caller can actually take.

This module answers the second question. It reads the dependent family at
system scope, so a dependent the caller cannot see still blocks the delete
rather than turning into the catalog's own rejection one line later, and names
only the dependents the caller is allowed to see.
"""

from services import product_read_authority

# The product store's own listing ceiling (services.product_store.list_records).
# Stating it here keeps the bound a decision rather than something a delete
# discovers by exceeding it.
MAX_DEPENDENT_SCAN = 1000

# The most dependents named in one message. Past a handful the list stops being
# an instruction and becomes a wall of text; the count still reports the rest.
MAX_NAMED = 5


def dependents(family: str, references, *, actor: str, role: str,
               name_field: str = "name") -> tuple[int, list[str]]:
    """How many records in *family* reference the target, and which of those
    the caller may see, by name.

    *references* is called with one document and says whether it points at the
    record being deleted. Returns ``(total, names)``; ``total`` counts every
    dependent, ``names`` only the visible ones and at most ``MAX_NAMED``.

    ``(0, [])`` when the family has not moved to KaveonDB read authority: on
    the legacy PostgreSQL path a dependent row is governed by its own foreign
    key, so there is no catalog reference for this to anticipate.
    """
    if not product_read_authority.enabled(family):
        return 0, []
    total = sum(
        1 for document in product_read_authority.list_documents(
            family, "kaveon-system", "Admin", max_records=MAX_DEPENDENT_SCAN)
        if references(document)
    )
    if not total:
        return 0, []
    names = [
        str(document.get(name_field) or document.get("id") or "")
        for document in product_read_authority.list_documents(
            family, actor, role, max_records=MAX_DEPENDENT_SCAN)
        if references(document)
    ]
    return total, [name for name in names if name][:MAX_NAMED]


def message(total: int, names: list[str], subject: str, action: str) -> str:
    """One sentence saying what is in the way and what to do about it.

    A dependent the caller cannot see is counted but not named, so the message
    never discloses another user's private content while still accounting for
    every record that blocks the delete.
    """
    noun = subject if total == 1 else subject + "s"
    detail = ""
    if names:
        shown = ", ".join(names)
        hidden = total - len(names)
        detail = f" ({shown}" + (f" and {hidden} more" if hidden > 0 else "") + ")"
    elif total:
        detail = " (not visible to you)"
    return f"{total} {noun}{detail} still {'uses' if total == 1 else 'use'} this. {action}"
