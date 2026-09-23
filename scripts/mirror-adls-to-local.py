"""Resumably mirror Kaveon's ADLS containers onto local disk for offline use.

Authenticate once with Azure CLI for the storage account, then run this script.
It copies blobs only; it never writes to or deletes anything in ADLS.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
import os

from azure.core import MatchConditions
from azure.identity import AzureCliCredential
from azure.storage.blob import BlobServiceClient


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", required=True, help="ADLS Gen2 storage account name")
    parser.add_argument("--destination", type=Path, default=Path("data/adls-mirror"))
    parser.add_argument(
        "--containers", nargs="+", default=("product-transactions", "opensource"),
        help="Containers to mirror (default: product-transactions opensource)",
    )
    parser.add_argument("--workers", type=int, default=8)
    args = parser.parse_args()
    if not args.account.isalnum() or not 1 <= args.workers <= 32:
        parser.error("account must be alphanumeric and workers must be between 1 and 32")

    root = args.destination.resolve()
    credential = AzureCliCredential()
    service = BlobServiceClient(
        f"https://{args.account}.blob.core.windows.net", credential=credential
    )

    def copy_one(container_name: str, blob) -> tuple[int, bool]:
        # ADLS directory markers have no payload; the local filesystem creates
        # those directories when actual child blobs are downloaded.
        if blob.size == 0 and blob.name.endswith("/"):
            return 0, False
        target = (root / container_name / Path(*blob.name.split("/"))).resolve()
        if not target.is_relative_to(root):
            raise RuntimeError("blob path escapes the selected destination")

        parent = target.parent
        while not parent.exists() and parent != root / container_name:
            parent = parent.parent
        if parent.is_file():
            if parent.stat().st_size != 0:
                raise RuntimeError(f"non-empty local object blocks a directory under {container_name}")
            parent.unlink()  # Prior exports sometimes materialized empty ADLS markers as files.
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.is_file() and target.stat().st_size == blob.size:
            return blob.size, False

        client = service.get_blob_client(container_name, blob.name)
        stream = client.download_blob(
            etag=blob.etag,
            match_condition=MatchConditions.IfNotModified,
            max_concurrency=4,
        )
        temporary = target.with_name(target.name + ".kaveon-download")
        try:
            with temporary.open("wb") as output:
                stream.readinto(output)
            if temporary.stat().st_size != blob.size:
                raise RuntimeError(f"download size mismatch in container {container_name}")
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
        return blob.size, True

    total_bytes = copied_bytes = copied_objects = object_count = 0
    jobs = []
    for container_name in args.containers:
        blobs = list(service.get_container_client(container_name).list_blobs())
        # Ignore zero-byte directory markers even when ADLS omitted the trailing slash.
        names = {blob.name.rstrip("/") for blob in blobs}
        for blob in blobs:
            name = blob.name.rstrip("/")
            if blob.size == 0 and any(other.startswith(name + "/") for other in names):
                continue
            jobs.append((container_name, blob))

    print(f"Objects to verify or copy: {len(jobs)}; destination: {root}", flush=True)
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = [pool.submit(copy_one, container, blob) for container, blob in jobs]
        for index, future in enumerate(as_completed(futures), 1):
            size, copied = future.result()
            object_count += 1
            total_bytes += size
            if copied:
                copied_objects += 1
                copied_bytes += size
            if index % 100 == 0 or index == len(futures):
                print(
                    f"checked={index}/{len(futures)} copied={copied_objects} "
                    f"copied_GiB={copied_bytes / 1024**3:.2f}",
                    flush=True,
                )
    print(
        f"Mirror complete: objects={object_count} size_GiB={total_bytes / 1024**3:.2f} "
        f"new_GiB={copied_bytes / 1024**3:.2f}; rerun is safe to resume.",
        flush=True,
    )


if __name__ == "__main__":
    main()
