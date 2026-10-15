"""PostgreSQL-free publication of one compiled DLM generation."""

import logging
import re

from services import dlm_compiled_artifact, product_store

logger = logging.getLogger(__name__)


def _positive_revision(record: dict | None, label: str) -> int:
    revision = record.get("revision") if isinstance(record, dict) else None
    if type(revision) is not int or revision < 1:
        raise RuntimeError(f"KaveonDB {label} revision is invalid")
    return revision


# Enough to clear orphans left by repeated failures, few enough that a
# genuinely stuck store reports rather than loops.
MAX_VERSION_PROBES = 16

# How many generations one dataset keeps, and why it is bounded at all.
#
# `dlm_compiled_artifact.read` only ever serves the highest-version ready run
# at the definition's current revision, so every other run for that dataset is
# unservable the moment a new one lands — and nothing removed them. One record
# per recompile accumulated forever in a family that is read by LISTING it:
# `read`, `load_bulk_context` and the scan below each list `dlm_run`, and
# `product_store.list_records` answers 503 once a listing passes 1,000. So a
# deployment that recompiled enough times would have every compiled-artifact
# read and the whole context banner fail with "KaveonDB product list exceeds
# its configured bound", with nothing in the product able to bring it back
# under. The DLM is the primary path on the homepage, so that is the product.
#
# A couple of superseded generations are kept deliberately: they are what an
# investigation of a bad answer reads. Only the records are retired — the
# immutable artifact bytes are never deleted, so a run document already
# resolved by a reader in flight still reads its object.
#
# Be precise about what this bounds and what it does not. It bounds the growth
# that has no end: one record per recompile, forever, for a dataset nothing
# else ever touches. It does NOT bound the listing itself, because the listing
# is per OWNER across every dataset while this retention is per dataset — an
# owner holding more than 1,000 / KEEP_RUN_GENERATIONS datasets still reaches
# the ceiling, and every seeded dataset shares the one `system` owner. Staying
# under it past that point needs the Engine to filter a product listing by a
# document field, which it cannot yet do.
KEEP_RUN_GENERATIONS = 3
# The most records one publication retires. A long-neglected dataset comes
# down over several publications rather than in one oversized transaction;
# `product_store.transact` accepts at most 100 mutations.
MAX_RUN_PRUNE = 50


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
    existing_runs = []
    for record in product_store.list_records("dlm_run", owner, "Admin", max_records=1000):
        record_id = str(record.get("id") or "") if isinstance(record, dict) else ""
        match = re.fullmatch(re.escape(dataset_id) + r"-v([1-9][0-9]*)", record_id)
        if match:
            run_revision = _positive_revision(record, "DLM run")
            document = record.get("document")
            if not isinstance(document, dict) or document.get("definition_id") != dataset_id:
                raise RuntimeError("KaveonDB DLM run identity is invalid")
            highest_version = max(highest_version, int(match.group(1)))
            existing_runs.append((int(match.group(1)), record_id, run_revision, document))
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
    retired = _retire_superseded_runs(existing_runs, definition_revision, owner)
    return {
        "definition_id": dataset_id,
        "definition_revision": definition_revision,
        "run_id": run_id,
        "run_revision": 2,
        "artifact": compiled,
        "runs_retired": retired,
    }


def superseded_runs(existing_runs: list, definition_revision: int) -> list:
    """Which of this dataset's existing runs can never be served again.

    Called with the runs that were present *before* the generation above was
    published, so the new run is not among them and is never a candidate.

    A run is servable only at ``status == "ready"`` and only at the
    definition's current revision, and only the highest such version is ever
    chosen. So everything else is unservable for good: a run left `building`
    or `failed` by an interrupted publication, and a run pinned to a
    definition revision the dataset has moved past. The newest few servable
    generations are kept anyway, because a superseded generation is what an
    investigation of a bad answer reads.

    The highest version present at the scan is kept whatever its status,
    because that is the one that may still be in flight. A publication takes
    ``highest + 1``, so a concurrent one that has committed its `building`
    run and not yet its `ready` update is always holding the top version
    here — and retiring it would make that second commit fail with its
    immutable bytes already written. A `building` run genuinely stranded by
    an interrupted publication stops being the top version as soon as the
    next generation lands, so it is retired one publication later rather
    than not at all.

    Returned oldest first, so a backlog comes down from the far end.
    """
    if not existing_runs:
        return []
    kept = {max(version for version, _, _, _ in existing_runs)}
    kept.update(sorted(
        (version for version, _, _, document in existing_runs
         if document.get("status") == "ready"
         and document.get("definition_revision") == definition_revision),
        reverse=True,
    )[:max(KEEP_RUN_GENERATIONS - 1, 0)])
    return sorted((run for run in existing_runs if run[0] not in kept),
                  key=lambda run: run[0])[:MAX_RUN_PRUNE]


def _retire_superseded_runs(existing_runs: list, definition_revision: int,
                            owner: str) -> int | None:
    """Remove the unservable runs, and never fail the publication over it.

    The generation is already published and readable by the time this runs.
    A store that refuses these deletes — a concurrent publication moved a
    revision, say — leaves records the next publication retires instead, so
    raising here would turn a successful rebuild into a failed one.

    Returns how many were retired, or ``None`` when the attempt failed. Those
    are not the same reading and a zero would have conflated them: "nothing
    needed retiring" is the ordinary steady state, while an installation
    where this never succeeds is one heading for the listing ceiling.
    """
    doomed = superseded_runs(existing_runs, definition_revision)
    if not doomed:
        return 0
    try:
        product_store.transact([
            product_store.ProductMutation("delete", "dlm_run", record_id,
                                          expected_revision=run_revision)
            for _, record_id, run_revision, _ in doomed
        ], owner, "Admin")
    except Exception as error:
        logger.warning("dlm_run_retirement_failed count=%d type=%s status=%s",
                       len(doomed), type(error).__name__,
                       getattr(error, "status_code", None))
        return None
    return len(doomed)
