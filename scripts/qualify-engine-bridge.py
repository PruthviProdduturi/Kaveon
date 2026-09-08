"""Exercise the real API bridge client against an isolated native Engine.

Run with api/venv/Scripts/python.exe; fixture creation uses the qualification
Python environment. No cloud credentials or existing metadata are read/written.
"""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch
import httpx
from fastapi import HTTPException

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "api"))
from services import engine_bridge as bridge


def main():
    with tempfile.TemporaryDirectory(prefix="kaveon-bridge-test-") as temp:
        work = Path(temp)
        data = work / "data"
        data.mkdir()
        (work / "catalogs").mkdir()
        python = ROOT / "engine/qualification/venv/Scripts/python.exe"
        subprocess.run([str(python), "-c", "import sys,pyarrow as pa,pyarrow.parquet as pq; pq.write_table(pa.table({'x':[1,2,3]}),sys.argv[1])", str(data / "measurements.parquet")], check=True)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        catalog_token, bridge_token, other_token = [secrets.token_urlsafe(32) for _ in range(3)]
        env = {key: value for key, value in os.environ.items() if not key.startswith("KAVEON_")}
        env.update({"KAVEON_HTTP_PORT": str(port), "KAVEON_DATA_DIR": str(data), "KAVEON_CATALOG_DIR": str(work / "catalogs"),
                    "KAVEON_CATALOG_DATABASE_PATH": str(work / "catalog.db"), "KAVEON_CATALOG_ADMIN_TOKEN": catalog_token,
                    "KAVEON_SECURITY_JSON": json.dumps({"bridge_token": bridge_token, "principals": [{"token": other_token, "principal": "bob", "role": "analyst"}]})})
        binary = ROOT / "engine/target/debug/kaveon-server.exe"
        with (work / "server.log").open("w") as output:
            process = subprocess.Popen([str(binary), str(work / "absent.toml")], cwd=work, env=env, stdout=output, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            try:
                deadline = time.monotonic() + 20
                while True:
                    try:
                        if httpx.get(base + "/health", timeout=1).is_success:
                            break
                    except httpx.HTTPError:
                        pass
                    if process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError((work / "server.log").read_text())
                    time.sleep(0.1)
                with patch.dict(os.environ, {"KAVEON_ENGINE_URL": base, "KAVEON_ENGINE_CATALOG_TOKEN": catalog_token, "KAVEON_ENGINE_BRIDGE_TOKEN": bridge_token}):
                    source = {"id": "integration-source", "engine_catalog": "registered", "storage_type": "local", "storage_config": {"base_path": str(data)}, "adapter_type": "native", "lifecycle": "draft"}
                    created = bridge.sync_catalog(source, "alice")
                    assert created["catalog"]["id"] == "platform-integration-source"
                    assert created["changed"] and created["catalog"]["revision"] == 1
                    assert not bridge.sync_catalog(source, "alice")["changed"]
                    source["engine_catalog"] = "renamed"
                    assert bridge.sync_catalog(source, "alice", 1)["catalog"]["revision"] == 2
                    source["engine_catalog"] = "conflicting"
                    try:
                        bridge.sync_catalog(source, "alice", 1)
                        raise AssertionError("stale revision overwrote Engine catalog")
                    except HTTPException as error:
                        assert error.status_code == 409
                    result = bridge.execute("SELECT COUNT(*) FROM measurements", "kaveon", "alice", "Analyst")
                    assert result["data"] == [[3]], result
                    query_path = base + "/v1/query/" + result["id"]
                    assert httpx.get(query_path, headers={"Authorization": "Bearer " + other_token}).status_code == 404
                    record = httpx.get(query_path, headers={"Authorization": "Bearer " + bridge_token, "x-kaveon-principal": "alice", "x-kaveon-role": "analyst"}).json()
                    assert record["context"]["principal"] == "alice"
                    try:
                        bridge.execute("SELECT COUNT(*) FROM measurements", "kaveon", "viewer", "Viewer")
                        raise AssertionError("Viewer delegated SQL")
                    except HTTPException as error:
                        assert error.status_code == 403
                    print(json.dumps({"catalog_create": "passed", "idempotent_retry": "passed", "revision_update": "passed", "stale_revision_rejected": "passed", "delegated_query": "passed", "principal_attribution": "passed", "query_isolation": "passed", "viewer_rejected": "passed"}, indent=2))
            finally:
                process.terminate()
                process.wait(timeout=10)


if __name__ == "__main__":
    main()
