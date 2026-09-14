from pathlib import Path
from types import SimpleNamespace

import pytest

from services import migration_checkpoint_store as checkpoints


def test_apply_fails_closed_without_durable_backend(monkeypatch, tmp_path):
    monkeypatch.delenv("KAVEON_MIGRATION_CHECKPOINT_MODE", raising=False)
    monkeypatch.delenv("KAVEON_ENVIRONMENT", raising=False)
    operation = SimpleNamespace(save=lambda *_args, **_kwargs: None)
    with pytest.raises(RuntimeError, match="requires KAVEON_MIGRATION_CHECKPOINT_MODE=adls"):
        checkpoints.run(operation, tmp_path / "state.json", apply=True, invoke=lambda _path: None)


def test_local_apply_requires_explicit_local_environment(monkeypatch, tmp_path):
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_MODE", "local")
    monkeypatch.setenv("KAVEON_ENVIRONMENT", "production")
    operation = SimpleNamespace(save=lambda *_args, **_kwargs: None)
    with pytest.raises(RuntimeError, match="explicit local environment"):
        checkpoints.run(operation, tmp_path / "state.json", apply=True, invoke=lambda _path: None)


def test_adls_hydrates_and_publishes_every_save(monkeypatch, tmp_path):
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_MODE", "adls")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT", "account")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER", "container")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX", "retirement/run-1")
    initial = b'{"checkpoint_sha256":"initial"}'

    class Store:
        writes = []

        def __init__(self, *_args, **_kwargs):
            pass

        def read(self, key, _maximum):
            assert key == "state.json"
            return initial, '"etag-1"'

        def write(self, key, value, etag):
            self.writes.append((key, value, etag))
            return f'"etag-{len(self.writes) + 1}"'

    monkeypatch.setattr(checkpoints, "AzureCheckpointStore", Store)

    def save(path: Path, value: bytes):
        checkpoints._atomic_local_write(Path(path), value)

    operation = SimpleNamespace(save=save, MAX_CHECKPOINT_BYTES=1024)

    def invoke(path):
        assert path.read_bytes() == initial
        operation.save(path, b"first")
        operation.save(path, b"second")
        return "complete"

    result = checkpoints.run(operation, tmp_path / "state.json", apply=True, invoke=invoke)
    assert result == "complete"
    assert Store.writes == [
        ("state.json", b"first", '"etag-1"'),
        ("state.json", b"second", '"etag-2"'),
    ]


def test_adls_refuses_untracked_local_checkpoint(monkeypatch, tmp_path):
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_MODE", "adls")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT", "account")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER", "container")
    monkeypatch.setenv("KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX", "retirement/run-1")
    checkpoint = tmp_path / "state.json"
    checkpoint.write_text("local-only")

    class EmptyStore:
        def __init__(self, *_args, **_kwargs):
            pass

        def read(self, *_args):
            return None

    monkeypatch.setattr(checkpoints, "AzureCheckpointStore", EmptyStore)
    operation = SimpleNamespace(save=lambda *_args, **_kwargs: None)
    with pytest.raises(RuntimeError, match="without a durable ADLS checkpoint"):
        checkpoints.run(operation, checkpoint, apply=True, invoke=lambda _path: None)
