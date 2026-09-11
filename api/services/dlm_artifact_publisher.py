"""Create-only publication of immutable DLM artifacts through an injected client."""

import hashlib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Protocol


MAX_ARTIFACT_BYTES = 16 * 1024 * 1024


class ImmutableArtifactClient(Protocol):
    def create_if_absent(self, path: str, content: bytes) -> None:
        """Create path conditionally; it must never overwrite an existing object."""

    def read(self, path: str, max_bytes: int) -> bytes | None:
        """Return at most max_bytes, or None when the path does not exist."""


@dataclass(frozen=True)
class PublishResult:
    path: str
    sha256: str
    bytes: int
    disposition: str


class Publisher:
    def __init__(self, client: ImmutableArtifactClient, artifact_root: Path):
        self._client = client
        self._root = artifact_root.resolve()

    def publish(self, path: str, expected_sha256: str) -> PublishResult:
        relative = PurePosixPath(path)
        if (not path or path.startswith("/") or "\\" in path or ":" in path
                or any(part in {"", ".", ".."} for part in relative.parts)):
            raise RuntimeError("DLM artifact path is invalid")
        source = self._root / Path(path)
        if not source.is_file():
            raise RuntimeError("DLM artifact source is missing")
        content = source.read_bytes()
        if len(content) > MAX_ARTIFACT_BYTES:
            raise RuntimeError("DLM artifact exceeds its byte bound")
        actual = hashlib.sha256(content).hexdigest()
        if actual != expected_sha256:
            raise RuntimeError("DLM artifact source hash mismatch")
        disposition = "created"
        try:
            self._client.create_if_absent(path, content)
        except Exception:
            remote = self._client.read(path, MAX_ARTIFACT_BYTES + 1)
            if remote != content:
                raise
            disposition = "already_present"
        remote = self._client.read(path, MAX_ARTIFACT_BYTES + 1)
        if remote != content:
            raise RuntimeError("Published DLM artifact failed exact reconciliation")
        return PublishResult(path, actual, len(content), disposition)
