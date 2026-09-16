"""Stream the ClickBench `hits` Parquet object into the lake.

Runs inside the cluster VNet (the storage firewall allows it there). The
public object is read over HTTPS in 64 MiB blocks and each block is put
straight into ADLS, so the pod needs no disk for the 14.8 GB file. The upload
is committed with one Put Block List, verified by byte length, and the SHA-256
of the streamed bytes is printed for the catalog manifest. An existing blob of
the same length is left alone. The caller token comes from the environment
and is never logged.
"""
from __future__ import annotations

import hashlib
import os
import sys
import time

import requests

SOURCE = os.environ.get(
    "CLICKBENCH_SOURCE", "https://datasets.clickhouse.com/hits_compatible/hits.parquet"
)
ACCOUNT = os.environ["ADLS_ACCOUNT"]
CONTAINER = os.environ.get("ADLS_CONTAINER", "opensource")
BLOB = os.environ.get("ADLS_BLOB", "benchmarks/clickbench/hits.parquet")
BLOCK = 64 * 1024 * 1024
if not ACCOUNT.isalnum() or CONTAINER != "opensource":
    raise SystemExit("Unexpected upload destination")

lake = requests.Session()
lake.headers.update({"Authorization": "Bearer " + os.environ["ADLS_UPLOAD_TOKEN"],
                     "x-ms-version": "2023-11-03"})
url = f"https://{ACCOUNT}.blob.core.windows.net/{CONTAINER}/{BLOB}"


def existing_length() -> int | None:
    r = lake.head(url, timeout=60)
    if r.status_code == 404:
        return None
    r.raise_for_status()
    return int(r.headers["Content-Length"])


def put_block(block_id: str, chunk: bytes) -> None:
    for attempt in range(5):
        r = lake.put(f"{url}?comp=block&blockid={block_id}", data=chunk, timeout=600)
        if r.status_code in (201, 202):
            return
        time.sleep(2 * (attempt + 1))
    raise SystemExit(f"block {block_id} failed: HTTP {r.status_code}")


def main() -> int:
    source = requests.get(SOURCE, stream=True, timeout=120)
    source.raise_for_status()
    expected = int(source.headers.get("Content-Length", "0")) or None
    have = existing_length()
    if have is not None and have == expected:
        print(f"present: {BLOB} {have} bytes")
        return 0
    digest = hashlib.sha256()
    ids: list[str] = []
    total = 0
    started = time.time()
    buffer = bytearray()
    for piece in source.iter_content(chunk_size=8 * 1024 * 1024):
        buffer.extend(piece)
        while len(buffer) >= BLOCK:
            chunk = bytes(buffer[:BLOCK])
            del buffer[:BLOCK]
            block_id = hashlib.sha256(f"{len(ids):08d}".encode()).hexdigest()[:32]
            put_block(block_id, chunk)
            digest.update(chunk)
            ids.append(block_id)
            total += len(chunk)
            if len(ids) % 10 == 0:
                rate = total / max(time.time() - started, 1e-9) / 1e6
                print(f"{total / 1e9:.2f} GB at {rate:.0f} MB/s", flush=True)
    if buffer:
        chunk = bytes(buffer)
        block_id = hashlib.sha256(f"{len(ids):08d}".encode()).hexdigest()[:32]
        put_block(block_id, chunk)
        digest.update(chunk)
        ids.append(block_id)
        total += len(chunk)
    if expected is not None and total != expected:
        raise SystemExit(f"streamed {total} bytes, source declared {expected}")
    body = "<?xml version='1.0' encoding='utf-8'?><BlockList>" + "".join(
        f"<Latest>{block_id}</Latest>" for block_id in ids
    ) + "</BlockList>"
    r = lake.put(f"{url}?comp=blocklist", data=body,
                 headers={"Content-Type": "application/xml",
                          "x-ms-blob-content-type": "application/octet-stream"},
                 timeout=600)
    r.raise_for_status()
    if existing_length() != total:
        raise SystemExit("committed length does not match streamed length")
    print(f"uploaded: {BLOB} {total} bytes sha256:{digest.hexdigest()} "
          f"in {time.time() - started:.0f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
