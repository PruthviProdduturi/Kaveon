"""Upload the built telemetry tables to the OpenSource snapshot prefix.

Runs where the storage firewall allows it (an AKS pod in the cluster VNet).
Uses a short-lived caller token from the environment, never logged. Large
files go up as 64 MiB blocks and are committed with one Put Block List; every
upload is verified by size, and an existing blob of the same size is skipped.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
import time
from pathlib import Path

import requests

ROOT = Path(os.environ.get("KAVEON_EVENTS_ROOT", "/work"))
ACCOUNT = os.environ["ADLS_ACCOUNT"]
CONTAINER = os.environ.get("ADLS_CONTAINER", "opensource")
PREFIX = os.environ.get("ADLS_PREFIX", "snapshots/2026-09-09-v1")
BLOCK = 64 * 1024 * 1024
if not ACCOUNT.isalnum() or CONTAINER != "opensource":
    raise SystemExit("Unexpected upload destination")

session = requests.Session()
session.headers.update({"Authorization": "Bearer " + os.environ["ADLS_UPLOAD_TOKEN"],
                        "x-ms-version": "2023-11-03"})
base = f"https://{ACCOUNT}.blob.core.windows.net/{CONTAINER}"


def head(url: str) -> int | None:
    r = session.head(url, timeout=60)
    if r.status_code == 404:
        return None
    r.raise_for_status()
    return int(r.headers["Content-Length"])


def put_blocks(url: str, path: Path) -> None:
    size = path.stat().st_size
    ids = []
    with path.open("rb") as f:
        n = 0
        while True:
            chunk = f.read(BLOCK)
            if not chunk:
                break
            block_id = hashlib.sha256(f"{n:08d}".encode()).hexdigest()[:32]
            for attempt in range(5):
                r = session.put(f"{url}?comp=block&blockid={block_id}", data=chunk, timeout=600)
                if r.status_code in (201, 202):
                    break
                time.sleep(2 * (attempt + 1))
            else:
                raise SystemExit(f"block {n} failed: HTTP {r.status_code}")
            ids.append(block_id)
            n += 1
            if n % 10 == 0:
                print(f"  {path.name}: {n * BLOCK / 1e9:.1f} GB of {size / 1e9:.1f} GB", flush=True)
    body = "<?xml version='1.0' encoding='utf-8'?><BlockList>" + "".join(f"<Latest>{i}</Latest>" for i in ids) + "</BlockList>"
    r = session.put(f"{url}?comp=blocklist", data=body.encode(), timeout=600,
                    headers={"x-ms-blob-content-type": "application/octet-stream"})
    if r.status_code != 201:
        raise SystemExit(f"block list failed: HTTP {r.status_code} {r.text[:300]}")


uploaded = []
for path in sorted(ROOT.rglob("*")):
    if not path.is_file() or path.suffix not in (".parquet", ".json"):
        continue
    relative = path.relative_to(ROOT).as_posix()
    url = f"{base}/{PREFIX}/{relative}"
    size = path.stat().st_size
    existing = head(url)
    if existing == size:
        print(f"skip {relative} ({size} bytes already present)", flush=True)
        uploaded.append({"path": relative, "bytes": size, "status": "present"})
        continue
    print(f"upload {relative} ({size / 1e9:.2f} GB)", flush=True)
    put_blocks(url, path)
    if head(url) != size:
        raise SystemExit(f"size mismatch after upload: {relative}")
    uploaded.append({"path": relative, "bytes": size, "status": "uploaded"})

print(json.dumps({"account": ACCOUNT, "container": CONTAINER, "prefix": PREFIX, "files": uploaded}, indent=2))
