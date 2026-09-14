"""PostgreSQL-free publication of one compiled DLM generation."""

import re

from services import dlm_compiled_artifact, product_store


def _positive_revision(record: dict | None, label: str) -> int:
    revision = record.get("revision") if isinstance(record, dict) else None
    if type(revision) is not int or revision < 1:
        raise RuntimeError(f"KaveonDB {label} revision is invalid")
    return revision


def publish(payload: dict, actor: str) -> dict:
    """Publish bytes, then atomically bind definition and terminal run records.

    Immutable bytes intentionally precede the metadata transaction. A failed
    transaction can leave an unreferenced object, but can never expose a ready
    run pointing at missing or divergent bytes.
    """
    dataset_id = str(payload.get("dataset_id") or "") if isinstance(payload, dict) else ""
    if not dataset_id.isdecimal() or not actor:
        raise RuntimeError("DLM generation requires dataset and actor identity")
    if "version" in payload:
        raise RuntimeError("DLM generation version is assigned by KaveonDB")

    dataset = product_store.read("dataset", dataset_id, actor, "Admin")
    if not dataset or not isinstance(dataset.get("document"), dict):
        raise RuntimeError("KaveonDB dataset is missing before DLM generation")
    dataset_revision = _positive_revision(dataset, "dataset")
    owner = str(dataset["document"].get("created_by") or "")
    if not owner or owner != actor:
        raise RuntimeError("Only the KaveonDB dataset owner can publish its DLM generation")

    definition_document = {"dataset_id": dataset_id, "dataset_revision": dataset_revision}
    definition = product_store.read("dlm_definition", dataset_id, owner, "Admin")
    mutations = []
    if definition is None:
        definition_revision = 1
        mutations.append(product_store.ProductMutation(
            "create", "dlm_definition", dataset_id, definition_document,
        ))
    else:
        current_revision = _positive_revision(definition, "DLM definition")
        if definition.get("document") == definition_document:
            definition_revision = current_revision
        else:
            definition_revision = current_revision + 1
            mutations.append(product_store.ProductMutation(
                "update", "dlm_definition", dataset_id, definition_document,
                expected_revision=current_revision,
            ))

    highest_version = 0
    for record in product_store.list_records("dlm_run", owner, "Admin", max_records=1000):
        record_id = str(record.get("id") or "") if isinstance(record, dict) else ""
        match = re.fullmatch(re.escape(dataset_id) + r"-v([1-9][0-9]*)", record_id)
        if match:
            _positive_revision(record, "DLM run")
            document = record.get("document")
            if not isinstance(document, dict) or document.get("definition_id") != dataset_id:
                raise RuntimeError("KaveonDB DLM run identity is invalid")
            highest_version = max(highest_version, int(match.group(1)))
    version = highest_version + 1
    compiled = dlm_compiled_artifact.publish({**payload, "version": version})
    if compiled is None:
        raise RuntimeError("Immutable DLM artifact publication is disabled")

    run_id = f"{dataset_id}-v{version}"
    building = {
        "definition_id": dataset_id,
        "definition_revision": definition_revision,
        "status": "building",
        "artifact": None,
    }
    ready = {
        **building,
        "status": "ready",
        "artifact": {"path": compiled["path"], "sha256": compiled["sha256"]},
    }
    mutations.extend((
        product_store.ProductMutation("create", "dlm_run", run_id, building),
        product_store.ProductMutation(
            "update", "dlm_run", run_id, ready, expected_revision=1,
        ),
    ))
    product_store.transact(mutations, owner, "Admin")
    return {
        "definition_id": dataset_id,
        "definition_revision": definition_revision,
        "run_id": run_id,
        "run_revision": 2,
        "artifact": compiled,
    }
