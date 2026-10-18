"""Immutable compiled-DLM publication and PostgreSQL-free retrieval."""

import hashlib
import json
import os
import re
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from services import adls_artifact_client, product_outbox, product_store


class VersionOccupied(RuntimeError):
    """This version's object already holds different bytes.

    Artifacts are write-once, and a version's path is derived from the run
    version, which in turn comes from the runs already recorded. So a
    publication that writes its bytes and then fails to commit its run leaves
    an object no run references — and every retry picks that same version,
    builds bytes that differ (if only by `built_at`), and finds the slot taken.
    Reported distinctly so the caller can move to the next version instead of
    being wedged on an orphan forever; the bytes are never overwritten.
    """


class DefinitionUnavailable(RuntimeError):
    """No compiled definition describes this dataset at its current revision.

    Distinguished from the module's other failures because it is the expected
    input to generation rather than a fault: a dataset that has never been
    compiled, or whose artifact predates its current revision, has no artifact
    to serve and needs one built. Serving still fails closed on it — a
    RuntimeError subclass, so every caller that treats this module as
    fail-closed keeps doing so — while the generator can recognise it and
    compile from scratch instead of refusing to start.
    """


MAX_BYTES = 16 * 1024 * 1024
LIVE_PUBLISH_KEY = "KAVEON_DLM_LIVE_ARTIFACT_PUBLISH_ENABLED"
_FIELDS = frozenset({"dataset_id", "version", "manifest", "stats_rollup", "usage_rollup",
                     "source_hash", "built_at", "status", "values_indexed"})
_RETIREMENT_FIELDS = _FIELDS | {"compiled_context"}


def _valid_fields(payload: dict) -> bool:
    return set(payload) in (_FIELDS, _RETIREMENT_FIELDS)


def _canonical(value: dict) -> bytes:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_BYTES:
        raise RuntimeError("Compiled DLM artifact exceeds its byte bound")
    return encoded


# One storage client per process, built on first use.
#
# A fresh client means a fresh DefaultAzureCredential, and a fresh credential
# has an empty token cache — so every compiled-artifact read re-ran the whole
# workload-identity exchange before it could fetch anything. Measured in the
# cluster: 0.24s to obtain the token against 0.045s to read the object. Nine
# datasets on the home page paid that nine times, which is two seconds of the
# banner spent proving who we are to a service we had just finished talking
# to. The credential caches tokens and renews them itself once it is kept.
_CLIENT_LOCK = threading.Lock()
_CLIENT = None


def _client():
    if os.getenv("KAVEON_LOCAL_PRODUCT_MODE", "").strip().lower() == "true":
        return LocalArtifactClient(os.environ["KAVEON_LOCAL_DLM_ARTIFACT_PATH"])
    global _CLIENT
    with _CLIENT_LOCK:
        if _CLIENT is None:
            _CLIENT = adls_artifact_client.AzureArtifactClient.from_env()
        return _CLIENT


def reset_client() -> None:
    """Drop the shared client. For tests, and for a credential rotation."""
    global _CLIENT
    with _CLIENT_LOCK:
        _CLIENT = None


class LocalArtifactClient:
    """Create-only immutable DLM objects for the explicitly local Docker mode."""
    def __init__(self, root: str):
        self.root = Path(root).resolve()

    def _path(self, key: str) -> Path:
        if (not isinstance(key, str) or not key or key.startswith("/")
                or any(part in {"", ".", ".."} for part in key.split("/"))):
            raise RuntimeError("Local DLM artifact path is invalid")
        target = (self.root / Path(*key.split("/"))).resolve()
        if not target.is_relative_to(self.root):
            raise RuntimeError("Local DLM artifact path escapes its root")
        return target

    def create_if_absent(self, key: str, content: bytes) -> None:
        target = self._path(key)
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            with target.open("xb") as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
        except FileExistsError:
            return

    def read(self, key: str, max_bytes: int) -> bytes | None:
        target = self._path(key)
        try:
            with target.open("rb") as stream:
                content = stream.read(max_bytes + 1)
        except FileNotFoundError:
            return None
        if len(content) > max_bytes:
            raise RuntimeError("Local DLM artifact exceeds its read bound")
        return content


def publish(payload: dict) -> dict | None:
    """Create and reconcile immutable bytes; disabled mode preserves legacy builds."""
    if os.getenv(LIVE_PUBLISH_KEY) != "true":
        return None
    if not _valid_fields(payload) or payload.get("status") != "ready":
        raise RuntimeError("Compiled DLM artifact payload is invalid")
    context = payload.get("compiled_context")
    if context is not None and (not isinstance(context, dict)
            or set(context) != {"values", "answers", "sketches", "router", "curation"}
            or not isinstance(context["values"], list) or not isinstance(context["answers"], list)
            or not isinstance(context["sketches"], list) or not isinstance(context["router"], dict)
            or not isinstance(context["curation"], dict)):
        raise RuntimeError("Compiled DLM context payload is invalid")
    dataset_id, version = str(payload.get("dataset_id") or ""), payload.get("version")
    if not dataset_id.isdecimal() or type(version) is not int or version < 1:
        raise RuntimeError("Compiled DLM artifact identity is invalid")
    content = _canonical(payload)
    digest = hashlib.sha256(content).hexdigest()
    path = f"dlm/{dataset_id}/v{version}/compiled.json"
    client = _client()
    try:
        client.create_if_absent(path, content)
    except Exception:
        # The write is create-only, so a failure here is either a genuine fault
        # or this exact publication already landed and is being retried.
        if client.read(path, MAX_BYTES + 1) is None:
            raise
    # The read-back is what settles it, because the two clients report an
    # occupied key differently: ADLS refuses the conditional write, while the
    # local client simply does not write. Comparing what is stored covers both,
    # and is the integrity check this publication needs regardless.
    stored = client.read(path, MAX_BYTES + 1)
    if stored is None:
        raise RuntimeError("Compiled DLM artifact vanished immediately after publication")
    if stored != content:
        raise VersionOccupied(
            f"Compiled DLM artifact v{version} already holds different bytes")
    return {"path": path, "sha256": digest, "bytes": len(content), "version": version}


def enqueue_run(transaction, dataset_id: str, owner: str, actor: str,
                definition_revision: int, artifact: dict) -> None:
    record_id = f"{dataset_id}-v{artifact['version']}"
    product_outbox.enqueue(transaction, family="dlm_runs", operation="create",
        record_id=record_id, payload={"definition_id": dataset_id,
            "definition_revision": definition_revision, "status": "ready",
            "artifact": {"path": artifact["path"], "sha256": artifact["sha256"]}},
        actor=actor, owner=owner)


class BulkContext:
    """The records a batch of artifact reads would otherwise fetch per dataset.

    `read` resolves one artifact by listing every DLM run and reading the
    dataset record. Called in a loop — which is what the context banner does,
    once per dataset — that is a full run listing and a dataset read each time.
    Nine datasets cost nine of both, and the banner took twenty-five seconds to
    decorate a strip above the composer.

    Loading them once and resolving against memory turns O(datasets) listings
    into three, leaving only the artifact content, which genuinely is per
    dataset.
    """

    __slots__ = ("datasets", "definitions", "runs")

    def __init__(self, datasets: dict, definitions: dict, runs: list):
        self.datasets = datasets
        self.definitions = definitions
        self.runs = runs


def load_bulk_context(actor: str) -> BulkContext:
    """Read the dataset, definition and run records once, for a batch of reads.

    The three listings are independent, and each costs about half a second of
    catalog round trips regardless of how few records it returns — so running
    them one after another spent a second and a half waiting rather than
    working. Nothing in the second listing depends on the first.
    """
    families = ("dataset", "dlm_definition", "dlm_run")
    with ThreadPoolExecutor(max_workers=len(families),
                            thread_name_prefix="dlm-bulk") as pool:
        listed = list(pool.map(
            lambda family: product_store.list_records(family, actor, "Admin",
                                                      max_records=1000),
            families,
        ))
    datasets = {str(record.get("id")): record
                for record in listed[0] if isinstance(record, dict)}
    definitions = {str(record.get("id")): record
                   for record in listed[1] if isinstance(record, dict)}
    runs = [record for record in listed[2] if isinstance(record, dict)]
    return BulkContext(datasets, definitions, runs)


def context_identity(context: BulkContext) -> str:
    """What the records in `context` say about compiled content, as a digest.

    This is a cache key for anything derived from the compiled artifacts. The
    catalog's own snapshot identity is not usable for that: it advances on
    every product write, and the platform records its own query history in the
    same catalog — so running one query would invalidate a cached answer that
    could not possibly have changed. This moves only when a dataset's
    revision, its definition's revision, or one of its runs does.
    """
    material = [
        [(dataset_id, context.datasets[dataset_id].get("revision"))
         for dataset_id in sorted(context.datasets)],
        [(dataset_id, context.definitions[dataset_id].get("revision"))
         for dataset_id in sorted(context.definitions)],
        sorted((str(record.get("id") or ""), record.get("revision"),
                (record.get("document") or {}).get("status"))
               for record in context.runs),
    ]
    encoded = json.dumps(material, sort_keys=True, separators=(",", ":"),
                         default=str).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


# The fields of a dataset that decide what a compiled context means.
#
# A context is compiled against the dataset's shape: which table it reads,
# which columns exist, which are dimensions and which are metrics, the filters
# that bound it and the column that carries time. Everything else on the
# record -- its name, its description, its visibility, who last touched it and
# when -- can change without making a single compiled cell wrong.
#
# This exists because the rule used to be the record's revision, so any write
# at all invalidated the context. The freshness sweep refreshes datasets by
# design, which meant the job that keeps a context current was also the thing
# that made it unservable: two of nine datasets were in that state, the
# billion-row telemetry table among them, each with a ready run at the right
# definition revision that nothing would serve.
DATASET_SHAPE_FIELDS = (
    "columns", "dimensions", "metrics", "filters", "date_column",
    "database_name", "schema_name", "table_name", "sql_text", "tables_used",
    "source",
)


def dataset_shape_key(document: dict) -> str:
    """A stable digest of the fields a compiled context depends on."""
    if not isinstance(document, dict):
        raise RuntimeError("Compiled DLM dataset document is invalid")
    material = {field: document.get(field) for field in DATASET_SHAPE_FIELDS}
    encoded = json.dumps(material, sort_keys=True, separators=(",", ":"),
                         default=str, ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def read(dataset_id: str, actor: str, role: str,
         *, context: BulkContext | None = None) -> dict | None:
    """Resolve and verify the newest compiled run without a PostgreSQL fallback."""
    dataset_id = str(dataset_id or "")
    if not dataset_id.isdecimal() or not actor:
        raise RuntimeError("Compiled DLM read identity is invalid")
    if role not in {"Viewer", "Analyst", "Editor", "Admin"}:
        raise RuntimeError("Compiled DLM application role is invalid")
    # The bridge uses its server-held admin credential, then applies the same
    # product visibility rules as dataset routes. Passing Viewer/Analyst to the
    # storage layer would incorrectly reduce reads to owner-only records.
    dataset = (context.datasets.get(dataset_id) if context is not None
               else product_store.read("dataset", dataset_id, actor, "Admin"))
    if dataset is None:
        return None
    dataset_document = dataset.get("document")
    owner = dataset_document.get("created_by") if isinstance(dataset_document, dict) else None
    dataset_revision = dataset.get("revision")
    if not owner or type(dataset_revision) is not int or dataset_revision < 1:
        raise RuntimeError("Compiled DLM dataset authority is invalid")
    visibility = dataset_document.get("visibility") or "internal"
    if not (role == "Admin" or visibility == "published"
            or visibility == "internal" and role in {"Analyst", "Editor"}
            or visibility == "private" and owner == actor):
        return None
    definition = (context.definitions.get(dataset_id) if context is not None
                  else product_store.read("dlm_definition", dataset_id, actor, "Admin"))
    # A definition matches on the dataset's shape when it records one, and on
    # the record revision when it does not. The revision rule is what every
    # definition written before this used, so it stays as the fallback rather
    # than invalidating every compiled context at once; a dataset adopts the
    # shape rule the next time its generation is published.
    definition_document = definition.get("document") if definition else None
    if not isinstance(definition_document, dict):
        raise DefinitionUnavailable("Compiled DLM definition is missing or stale")
    recorded_shape = definition_document.get("dataset_shape_sha256")
    if recorded_shape is not None:
        if (definition_document.get("dataset_id") != dataset_id
                or recorded_shape != dataset_shape_key(dataset_document)):
            raise DefinitionUnavailable("Compiled DLM definition is missing or stale")
    else:
        expected_definition = {"dataset_id": dataset_id, "dataset_revision": dataset_revision}
        if definition_document != expected_definition:
            raise DefinitionUnavailable("Compiled DLM definition is missing or stale")
    definition_revision = definition.get("revision")
    if type(definition_revision) is not int or definition_revision < 1:
        raise RuntimeError("Compiled DLM definition revision is invalid")
    candidates = []
    runs = (context.runs if context is not None
            else product_store.list_records("dlm_run", actor, "Admin", max_records=1000))
    for record in runs:
        document = record.get("document") if isinstance(record, dict) else None
        match = re.fullmatch(re.escape(dataset_id) + r"-v([1-9][0-9]*)", str(record.get("id") or ""))
        if (match and isinstance(document, dict) and document.get("definition_id") == dataset_id
                and document.get("definition_revision") == definition_revision
                and document.get("status") == "ready"):
            candidates.append((int(match.group(1)), document))
    if not candidates:
        return None
    version, run = max(candidates, key=lambda item: item[0])
    artifact = run.get("artifact")
    expected_path = f"dlm/{dataset_id}/v{version}/compiled.json"
    if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
            or artifact.get("path") != expected_path
            or re.fullmatch(r"[0-9a-f]{64}", str(artifact.get("sha256") or "")) is None):
        raise RuntimeError("Compiled DLM run artifact reference is invalid")
    content = _client().read(expected_path, MAX_BYTES + 1)
    if content is None or len(content) > MAX_BYTES or hashlib.sha256(content).hexdigest() != artifact["sha256"]:
        raise RuntimeError("Compiled DLM artifact is missing or corrupt")
    try:
        payload = json.loads(content)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("Compiled DLM artifact JSON is invalid") from error
    if (not isinstance(payload, dict) or not _valid_fields(payload)
            or payload.get("dataset_id") != dataset_id or payload.get("version") != version
            or payload.get("status") != "ready" or not isinstance(payload.get("manifest"), dict)
            or not isinstance(payload.get("stats_rollup"), dict)
            or not isinstance(payload.get("usage_rollup"), dict)):
        raise RuntimeError("Compiled DLM artifact content is invalid")
    return payload
