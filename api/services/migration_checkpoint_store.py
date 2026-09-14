"""Durable, fail-closed storage for PostgreSQL retirement checkpoints.

Backfill modules retain their existing tamper-evident JSON formats.  This
runtime mirrors every atomic checkpoint save to ADLS and restores it before a
resume, so pod replacement cannot lose migration progress.
"""

from __future__ import annotations

import hashlib
import os
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

from azure.identity import DefaultAzureCredential


_LOCAL_ENVIRONMENTS = {"local", "dev", "development", "test"}


class AzureCheckpointStore:
    """Optimistically-concurrent ADLS block-blob checkpoint store."""

    def __init__(self, account: str, container: str, prefix: str, *, credential=None, opener=urlopen):
        if not account or not container or not prefix.strip("/"):
            raise RuntimeError("ADLS checkpoint account, container, and prefix are required")
        self.account = account
        self.container = container.strip("/")
        self.prefix = prefix.strip("/")
        self.credential = credential or DefaultAzureCredential()
        self._opener = opener

    def _url(self, key: str) -> str:
        path = quote(f"{self.prefix}/{key}", safe="/")
        return f"https://{self.account}.blob.core.windows.net/{self.container}/{path}"

    def _request(self, method: str, key: str, body: bytes | None = None, **headers):
        token = self.credential.get_token("https://storage.azure.com/.default").token
        request = Request(self._url(key), data=body, method=method, headers={
            "Authorization": f"Bearer {token}",
            "x-ms-version": "2023-11-03",
            "x-ms-date": __import__("email.utils", fromlist=["formatdate"]).formatdate(usegmt=True),
            **headers,
        })
        return self._opener(request)

    def read(self, key: str, max_bytes: int) -> tuple[bytes, str] | None:
        try:
            response = self._request("GET", key, Range=f"bytes=0-{max_bytes}")
        except HTTPError as error:
            if error.code == 404:
                return None
            raise RuntimeError(f"cannot read durable migration checkpoint: HTTP {error.code}") from error
        try:
            value = response.read(max_bytes + 1)
            etag = response.headers.get("ETag")
        finally:
            response.close()
        if len(value) > max_bytes or not etag:
            raise RuntimeError("durable migration checkpoint is oversized or missing an ETag")
        return value, etag

    def write(self, key: str, value: bytes, prior_etag: str | None) -> str:
        digest = hashlib.sha256(value).hexdigest()
        version_key = f"{key}.versions/{digest}.json"
        try:
            response = self._request("PUT", version_key, value, **{
                "Content-Length": str(len(value)), "Content-Type": "application/json",
                "x-ms-blob-type": "BlockBlob", "If-None-Match": "*",
            })
            response.close()
        except HTTPError as error:
            if error.code not in (409, 412):
                raise RuntimeError(f"cannot preserve checkpoint version: HTTP {error.code}") from error

        condition = {"If-Match": prior_etag} if prior_etag else {"If-None-Match": "*"}
        try:
            response = self._request("PUT", key, value, **{
                "Content-Length": str(len(value)), "Content-Type": "application/json",
                "x-ms-blob-type": "BlockBlob", **condition,
            })
            etag = response.headers.get("ETag")
            response.close()
        except HTTPError as error:
            if error.code in (409, 412):
                raise RuntimeError("durable migration checkpoint changed concurrently") from error
            raise RuntimeError(f"cannot write durable migration checkpoint: HTTP {error.code}") from error
        if not etag:
            raise RuntimeError("durable migration checkpoint write returned no ETag")
        confirmed = self.read(key, len(value))
        if confirmed is None or confirmed[0] != value or confirmed[1] != etag:
            raise RuntimeError("durable migration checkpoint read-after-write verification failed")
        return etag


def _atomic_local_write(path: Path, value: bytes) -> None:
    import tempfile
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="wb", dir=path.parent, prefix=path.name + ".", delete=False) as handle:
            temporary = Path(handle.name)
            os.chmod(temporary, 0o600)
            handle.write(value)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


@contextmanager
def durable_checkpoint(operation, checkpoint: Path, *, apply: bool):
    """Hydrate a checkpoint and publish after every operation-level save."""
    mode = os.getenv("KAVEON_MIGRATION_CHECKPOINT_MODE", "").strip().lower()
    environment = os.getenv("KAVEON_ENVIRONMENT", "").strip().lower()
    if not mode:
        mode = "local" if not apply else ""
    if mode == "local":
        if apply and environment not in _LOCAL_ENVIRONMENTS:
            raise RuntimeError("local migration checkpoints are allowed only in an explicit local environment")
        yield checkpoint
        return
    if mode != "adls":
        raise RuntimeError("apply requires KAVEON_MIGRATION_CHECKPOINT_MODE=adls (or explicit local development)")

    store = AzureCheckpointStore(
        os.getenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT", ""),
        os.getenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER", ""),
        os.getenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX", ""),
    )
    key = checkpoint.name
    maximum = int(getattr(operation, "MAX_CHECKPOINT_BYTES", 64 * 1024 * 1024))
    remote = store.read(key, maximum)
    etag = remote[1] if remote else None
    if remote:
        _atomic_local_write(checkpoint.resolve(), remote[0])
    elif checkpoint.exists():
        raise RuntimeError("local checkpoint exists without a durable ADLS checkpoint")

    save_name = "save_checkpoint" if hasattr(operation, "save_checkpoint") else "save"
    original_save = getattr(operation, save_name)

    def save_and_publish(path, *args, **kwargs):
        nonlocal etag
        original_save(path, *args, **kwargs)
        value = Path(path).read_bytes()
        if len(value) > maximum:
            raise RuntimeError("migration checkpoint exceeds its configured byte bound")
        etag = store.write(key, value, etag)

    setattr(operation, save_name, save_and_publish)
    try:
        yield checkpoint
    finally:
        setattr(operation, save_name, original_save)


def run(operation, checkpoint: Path, *, apply: bool, invoke):
    """Execute one backfill with the configured durable checkpoint backend."""
    with durable_checkpoint(operation, checkpoint, apply=apply) as local_checkpoint:
        return invoke(local_checkpoint)
