"""`kaveon-up.sh` must never delete a key the operator put in the environment file.

It used to. The script wrote `.env` whole, reading back only the six secrets it
generates, so a re-run silently deleted everything else — and what it deleted
was exactly what docs/guides/self-hosting.md tells an operator to add by hand:
the OAuth provider id and secret, and `AUTH_ADMIN_EMAILS`. Losing the last of
those leaves a deployment with no administrator, and the same guide tells you
to re-run this script to restart and to upgrade. A real file went from 24 keys
to 13.

These tests drive the script's environment-file section directly. Everything
above it needs Docker and everything below it starts containers, so the section
is sliced out and run against a harness that supplies the variables the earlier
part would have computed. That keeps the test honest about the code that ships
rather than re-implementing it.
"""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / "scripts" / "kaveon-up.sh"

# A file shaped like a real deployment's: the six generated secrets, the three
# keys the self-hosting guide tells you to add, a second provider, the
# credential keys that decrypt registered sources, and Engine tuning.
OPERATOR_ENV = """\
# Managed block written by scripts/kaveon-up.sh on 2026-10-01T00:00:00Z.

NODE_ENV=production
KAVEON_SYSTEM_STORAGE=adls://kaveonlake/system/kaveon/system/v2
KAVEON_PRODUCT_STORAGE_MODE=adls
KAVEON_PRODUCT_ADLS_ACCOUNT=kaveonlake
KAVEON_PRODUCT_ADLS_CONTAINER=system
KAVEON_PRODUCT_ADLS_PREFIX=kaveon/system/v2
KAVEON_DATA_PATH=/opt/kaveon/data

AUTH_SECRET=aaaa1111
KAVEON_PROXY_SECRET=bbbb2222
KAVEON_ENGINE_ADMIN_TOKEN=cccc3333
KAVEON_ENGINE_BRIDGE_TOKEN=dddd4444
KAVEON_EXCHANGE_TOKEN=eeee5555
KAVEON_CATALOG_ADMIN_TOKEN=ffff6666

# Yours.
AUTH_MICROSOFT_ENTRA_ID_ID=00000000-1111-2222-3333-444444444444
AUTH_MICROSOFT_ENTRA_ID_SECRET=entra-secret-value
AUTH_MICROSOFT_ENTRA_ID_ISSUER=https://login.microsoftonline.com/common/v2.0
AUTH_ADMIN_EMAILS=operator@example.com
GITHUB_ID=github-client-id
GITHUB_SECRET=github-client-secret
KAVEON_CREDENTIAL_KEYS=v1:deadbeef
KAVEON_CREDENTIAL_ACTIVE_KEY=v1
AUTH_URL=https://kaveon.example.com
KAVEON_LOCAL_PARALLELISM=8
KAVEON_QUERY_MEMORY_LIMIT_BYTES=536870912
"""

HARNESS = """\
set -euo pipefail
ENV_FILE=".env"
STORAGE="adls://kaveonlake/system/kaveon/system/v2"
STORAGE_MODE="adls"
STORAGE_LOCAL_PATH="/var/lib/kaveon/product-transactions"
STORAGE_ACCOUNT="kaveonlake"
STORAGE_CONTAINER="system"
STORAGE_PREFIX="kaveon/system/v2"
DATA_PATH="${HARNESS_DATA_PATH:-}"
note() { printf '  %s\\n' "$*"; }
"""

def _working_bash() -> str | None:
    """A bash that can actually run a script.

    On Windows the first `bash` on PATH is often the WSL stub, which fails with
    a UTF-16 COM error instead of running anything, so each candidate is tried
    before it is trusted. Git for Windows ships a real one.
    """
    candidates = [
        shutil.which("bash"),
        Path(os.environ.get("PROGRAMFILES", "C:\\Program Files")) / "Git" / "bin" / "bash.exe",
        Path(os.environ.get("PROGRAMFILES(X86)", "C:\\Program Files (x86)")) / "Git" / "bin" / "bash.exe",
        "/bin/bash",
        "/usr/bin/bash",
    ]
    for candidate in candidates:
        if not candidate or not Path(candidate).exists():
            continue
        try:
            probe = subprocess.run([candidate, "-c", "echo ok"], capture_output=True,
                                   text=True, timeout=60)
        except OSError:
            continue
        if probe.returncode == 0 and "ok" in probe.stdout:
            return candidate
    return None


BASH = _working_bash()

pytestmark = pytest.mark.skipif(BASH is None,
                                reason="no working bash; kaveon-up.sh is a bash script")


def _env_section() -> str:
    """The script's secrets and environment-file section, verbatim.

    Sliced by its own section comment and its closing note so the test runs the
    shipped code. If either marker moves, this raises rather than silently
    testing nothing.
    """
    lines = SCRIPT.read_text(encoding="utf-8").splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith("# ── Secrets"))
        end = next(i for i, line in enumerate(lines) if "of your own keys kept" in line)
    except StopIteration:  # pragma: no cover - a guard, not a path
        raise AssertionError(
            "kaveon-up.sh no longer has the secrets section or the environment-file "
            "note this test slices between; update the markers rather than deleting "
            "the test.")
    return "\n".join(lines[start:end + 1])


def _run(tmp_path: Path, existing: str | None, data_path: str | None = None) -> dict[str, str]:
    if existing is not None:
        (tmp_path / ".env").write_text(existing, encoding="utf-8")
    script = tmp_path / "harness.sh"
    script.write_text(HARNESS + _env_section(), encoding="utf-8")
    environment = dict(os.environ)
    environment.pop("HARNESS_DATA_PATH", None)
    if data_path:
        environment["HARNESS_DATA_PATH"] = data_path
    result = subprocess.run([BASH, str(script)], cwd=tmp_path, capture_output=True,
                            text=True, env=environment, timeout=120)
    assert result.returncode == 0, result.stderr
    return _keys((tmp_path / ".env").read_text(encoding="utf-8"))


def _keys(text: str) -> dict[str, str]:
    pairs = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        pairs[key] = value
    return pairs


def test_a_rerun_keeps_every_key_the_operator_added(tmp_path):
    before = _keys(OPERATOR_ENV)
    after = _run(tmp_path, OPERATOR_ENV)
    lost = sorted(set(before) - set(after))
    assert not lost, f"a re-run deleted {lost}"


def test_a_rerun_changes_no_value(tmp_path):
    before = _keys(OPERATOR_ENV)
    after = _run(tmp_path, OPERATOR_ENV)
    changed = {key: (before[key], after[key]) for key in before
               if key in after and before[key] != after[key]}
    assert not changed, f"a re-run rewrote {changed}"


@pytest.mark.parametrize("key", [
    # Without this one nobody is an administrator, which is the worst of the
    # failures because the deployment still looks healthy.
    "AUTH_ADMIN_EMAILS",
    "AUTH_MICROSOFT_ENTRA_ID_ID",
    "AUTH_MICROSOFT_ENTRA_ID_SECRET",
    "GITHUB_SECRET",
    # Registered source credentials cannot be decrypted without this.
    "KAVEON_CREDENTIAL_KEYS",
    # The workers come up with nothing to read without this.
    "KAVEON_DATA_PATH",
    "KAVEON_LOCAL_PARALLELISM",
])
def test_the_keys_whose_loss_breaks_a_deployment_survive(tmp_path, key):
    assert key in _run(tmp_path, OPERATOR_ENV)


def test_generated_secrets_are_not_regenerated(tmp_path):
    """Re-running must not invalidate live sessions or the Studio-to-API trust."""
    after = _run(tmp_path, OPERATOR_ENV)
    assert after["AUTH_SECRET"] == "aaaa1111"
    assert after["KAVEON_PROXY_SECRET"] == "bbbb2222"
    assert after["KAVEON_ENGINE_BRIDGE_TOKEN"] == "dddd4444"


def test_a_data_path_already_in_the_file_survives_a_run_without_data(tmp_path):
    """`--data` is optional on a re-run. Dropping the mount silently left the
    workers with no tables, which reads as an empty deployment rather than a
    missing flag."""
    after = _run(tmp_path, OPERATOR_ENV)
    assert after["KAVEON_DATA_PATH"] == "/opt/kaveon/data"


def test_an_explicit_data_path_wins_over_the_file(tmp_path):
    after = _run(tmp_path, OPERATOR_ENV, data_path="/mnt/warehouse")
    assert after["KAVEON_DATA_PATH"] == "/mnt/warehouse"


def test_a_first_run_writes_the_sign_in_hints(tmp_path):
    """With no file to carry through, the commented provider keys are the only
    pointer a first-time operator gets toward requiring real sign-in."""
    script = tmp_path / "harness.sh"
    script.write_text(HARNESS + _env_section(), encoding="utf-8")
    environment = dict(os.environ)
    environment.pop("HARNESS_DATA_PATH", None)
    result = subprocess.run([BASH, str(script)], cwd=tmp_path, capture_output=True,
                            text=True, env=environment, timeout=120)
    assert result.returncode == 0, result.stderr
    written = (tmp_path / ".env").read_text(encoding="utf-8")
    assert "# AUTH_ADMIN_EMAILS=you@example.com" in written
    assert "AUTH_SECRET=" in written


def test_the_file_is_replaced_in_one_step(tmp_path):
    """A half-written environment file does not start a deployment, so the new
    file is moved into place rather than truncating the old one."""
    source = SCRIPT.read_text(encoding="utf-8")
    assert 'mv -f "$TEMP_ENV" "$ENV_FILE"' in source
    assert '} > "$ENV_FILE"' not in source, (
        "the environment file is being truncated in place again")
