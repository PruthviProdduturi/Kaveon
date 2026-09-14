"""Atomic publication of the ready-DLM definition migration event."""

from dataclasses import dataclass

from services import product_outbox, product_store


@dataclass(frozen=True)
class DefinitionPublication:
    event: object
    owner: str
    revision: int


def publish_ready(transaction, dataset_id: str, actor: str):
    """Fence readiness and its canonical outbox event in one source transaction."""
    dataset_id = str(dataset_id or "")
    if not dataset_id or not actor:
        raise RuntimeError("DLM definition publication requires dataset and actor identity")
    source = transaction.query_one(
        "SELECT d.created_by, a.status FROM datasets d "
        "JOIN dlm_artifact a ON a.dataset_id = CAST(d.id AS TEXT) "
        "WHERE d.id = @param0 FOR UPDATE", [int(dataset_id)],
    )
    if not source:
        raise RuntimeError("DLM definition source disappeared before publication")
    if source.get("status") != "ready":
        raise RuntimeError("DLM definition cannot publish before its artifact is ready")
    owner = str(source.get("created_by") or "")
    if not owner:
        raise RuntimeError("DLM definition source owner is missing")
    dataset = product_store.read("dataset", dataset_id, owner, "Admin")
    if not dataset or not isinstance(dataset.get("document"), dict):
        raise RuntimeError("KaveonDB dataset is missing before DLM definition publication")
    if str(dataset["document"].get("created_by") or "") != owner:
        raise RuntimeError("KaveonDB dataset ownership differs from the DLM definition source")
    revision = dataset.get("revision")
    if type(revision) is not int or revision < 1:
        raise RuntimeError("KaveonDB dataset revision is invalid")
    document = {"dataset_id": dataset_id, "dataset_revision": revision}
    existing = product_store.read("dlm_definition", dataset_id, owner, "Admin")
    operation = "create" if existing is None else "update"
    if existing is None:
        resulting_revision = 1
    else:
        current_revision = existing.get("revision")
        if type(current_revision) is not int or current_revision < 1:
            raise RuntimeError("KaveonDB DLM definition revision is invalid")
        resulting_revision = current_revision if existing.get("document") == document else current_revision + 1
    event = product_outbox.enqueue(
        transaction, family="dlm_definitions", operation=operation,
        record_id=dataset_id, payload=document, actor=actor, owner=owner,
    )
    return DefinitionPublication(event, owner, resulting_revision)
