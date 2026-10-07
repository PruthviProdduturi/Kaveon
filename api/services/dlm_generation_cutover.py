"""PostgreSQL-free publication of one compiled DLM generation."""

import re

from services import dlm_compiled_artifact, product_store


def _positive_revision(record: dict | None, label: str) -> int:
    revision = record.get("revision") if isinstance(record, dict) else None
    if type(revision) is not int or revision < 1:
        raise RuntimeError(f"KaveonDB {label} revision is invalid")
    return revision


# Enough to clear orphans left by repeated failures, few enough that a
# genuinely stuck store reports rather than loops.
MAX_VERSION_PROBES = 16


def publish(payload: dict, actor: str, role: str = "Viewer") -> dict:
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
    # Every record below is written as `owner`, never as the caller, so what
    # this guards is who may trigger a rebuild — not who the records belong to.
    # Requiring caller == owner left every seeded dataset permanently without
    # context: those are owned by `system`, which is not a principal anyone can
    # sign in as, so no one could ever build their DLM. An Admin may rebuild a
    # dataset they do not own; the artifact still belongs to the dataset.
    if not owner or (owner != actor and role != "Admin"):
        raise RuntimeError("Only the dataset owner or an Admin can publish its DLM generation")

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
    # A publication that wrote its bytes and then failed to commit its run
    # leaves an object no run references, and the version it used still looks
    # free. Step over any such orphan rather than overwriting it: the bytes
    # stay immutable and a dataset cannot be wedged by one failed attempt.
    version = highest_version + 1
    for _ in range(MAX_VERSION_PROBES):
        try:
            compiled = dlm_compiled_artifact.publish({**payload, "version": version})
            break
        except dlm_compiled_artifact.VersionOccupied:
            version += 1
    else:
        raise RuntimeError("No free compiled DLM artifact version for this dataset")
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
    # The run's two revisions are two commits, not one. A product commit
    # applies each change once against a single base snapshot, so KaveonDB
    # rejects a transaction that touches one record twice ("product record
    # 'dlm_run/<id>' is changed more than once") — which is why no DLM could
    # ever be published. dlm_run_backfill already builds the same lifecycle as
    # separate commits; this now matches it, so a run reaches revision 2 by the
    # same route however it was created.
    #
    # Splitting costs nothing a reader can observe. The bytes are already
    # published, and `read` only ever resolves a run whose status is ready, so
    # a failure between the two commits strands a building run that every
    # reader ignores and the next generation supersedes.
    mutations.append(product_store.ProductMutation("create", "dlm_run", run_id, building))
    product_store.transact(mutations, owner, "Admin")
    product_store.transact([product_store.ProductMutation(
        "update", "dlm_run", run_id, ready, expected_revision=1,
    )], owner, "Admin")
    return {
        "definition_id": dataset_id,
        "definition_revision": definition_revision,
        "run_id": run_id,
        "run_revision": 2,
        "artifact": compiled,
    }
