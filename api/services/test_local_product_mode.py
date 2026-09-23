"""The loopback local Docker profile uses KaveonDB without PostgreSQL."""
import pytest

from services import postgresql_retirement_runtime as runtime, product_read_authority
from services.postgresql_retirement_gate import AUTHORITY_FAMILIES


def configure(monkeypatch):
    monkeypatch.setenv(runtime.LOCAL_MODE_KEY, "true")
    monkeypatch.setenv(runtime.AUTHORITY_KEY, ",".join(AUTHORITY_FAMILIES))
    monkeypatch.setenv(product_read_authority.ENVIRONMENT_KEY,
                       ",".join(product_read_authority.SUPPORTED_FAMILIES))
    monkeypatch.setenv("KAVEON_ENGINE_URL", "http://engine-coordinator:8080")
    monkeypatch.setenv("KAVEON_ENGINE_PRIVATE_HTTP", "true")
    monkeypatch.setenv("KAVEON_ENGINE_BRIDGE_TOKEN", "local-bridge")
    monkeypatch.setenv("KAVEON_DLM_LIVE_ARTIFACT_PUBLISH_ENABLED", "true")
    monkeypatch.setenv("KAVEON_LOCAL_DLM_ARTIFACT_PATH", "/var/lib/kaveon/dlm")


def test_local_mode_uses_all_kaveondb_families_without_retirement_receipts(monkeypatch):
    configure(monkeypatch)
    result = runtime.validate()
    assert result["enabled"] is True
    assert result["authority"] == "kaveondb"
    assert result["phase"] == "local_development"
    assert result["authority_family_count"] == len(AUTHORITY_FAMILIES)


def test_local_mode_requires_every_product_read_family(monkeypatch):
    configure(monkeypatch)
    monkeypatch.setenv(product_read_authority.ENVIRONMENT_KEY, "datasets")
    with pytest.raises(RuntimeError, match="every product read family"):
        runtime.validate()


def test_local_artifact_client_is_create_only_and_bounded(tmp_path, monkeypatch):
    from services.dlm_compiled_artifact import LocalArtifactClient
    client = LocalArtifactClient(str(tmp_path))
    client.create_if_absent("dlm/3/v1/compiled.json", b"original")
    client.create_if_absent("dlm/3/v1/compiled.json", b"replacement")
    assert client.read("dlm/3/v1/compiled.json", 100) == b"original"
    with pytest.raises(RuntimeError, match="read bound"):
        client.read("dlm/3/v1/compiled.json", 2)
    with pytest.raises(RuntimeError, match="invalid"):
        client.read("../outside", 100)
