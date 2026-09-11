"""Build the fixed extended corpus and a private, create-only AKS upload bundle."""

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import tarfile
from urllib.parse import quote

import duckdb
import pyarrow.parquet as pq

from same_files import EXTENDED_QUERIES


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def content_md5(path):
    digest = hashlib.md5(usedforsecurity=False)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return base64.b64encode(digest.digest()).decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", required=True)
    parser.add_argument("--container", default="silver")
    parser.add_argument("--prefix", required=True, help="New, immutable prefix such as benchmarks/run-20260910-a1")
    parser.add_argument("--rows", type=int, default=5_000_000)
    parser.add_argument("--customers", type=int, default=100_000)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.rows < 1 or args.customers < 1 or args.customers > args.rows:
        parser.error("rows and customers must be positive and customers cannot exceed rows")
    if not args.prefix.startswith("benchmarks/") or not all(part and part not in {".", ".."} for part in args.prefix.split("/")):
        parser.error("prefix must be a new path below benchmarks/ without dot segments")
    if args.output.exists() and any(args.output.iterdir()):
        parser.error("output must be absent or empty; fixture bundles are immutable")

    data = args.output / "upload"
    data.mkdir(parents=True, exist_ok=True)
    db = duckdb.connect()
    tables = {
        "events": f"SELECT i::BIGINT event_id, (i % {args.customers})::BIGINT customer_id, (i % 17)::BIGINT category, (i % 1000)::BIGINT amount FROM range({args.rows}) t(i)",
        "customers": f"SELECT i::BIGINT customer_id FROM range({args.customers}) t(i)",
    }
    objects = []
    for name, sql in tables.items():
        folder = data / name
        log = folder / "_delta_log"
        log.mkdir(parents=True)
        parquet = folder / "data.parquet"
        table = db.execute(sql).to_arrow_table()
        pq.write_table(table, parquet, compression="snappy", row_group_size=16_384)
        schema = {"type": "struct", "fields": [
            {"name": field.name, "type": "long", "nullable": True, "metadata": {}}
            for field in table.schema
        ]}
        actions = [
            {"protocol": {"minReaderVersion": 1, "minWriterVersion": 2}},
            {"metaData": {"id": secrets.token_hex(16), "format": {"provider": "parquet", "options": {}},
                          "schemaString": json.dumps(schema), "partitionColumns": [], "configuration": {}}},
            {"add": {"path": "data.parquet", "partitionValues": {}, "size": parquet.stat().st_size,
                     "modificationTime": 0, "dataChange": True}},
        ]
        delta = log / "00000000000000000000.json"
        delta.write_text("\n".join(json.dumps(action, separators=(",", ":")) for action in actions) + "\n", encoding="utf-8")
        db.execute(f"CREATE VIEW {name} AS SELECT * FROM read_parquet('{parquet.as_posix()}')")
        for path in (parquet, delta):
            objects.append({"path": f"{args.prefix}/{path.relative_to(data).as_posix()}",
                            "sha256": sha256(path), "content_md5": content_md5(path), "bytes": path.stat().st_size,
                            "parquet_data": path == parquet})

    expected = {}
    for name, sql in EXTENDED_QUERIES.items():
        rows = [list(row) for row in db.execute(sql).fetchall()]
        encoded = json.dumps(rows, separators=(",", ":")).encode()
        expected[name] = {"sql": sql, "result_rows": len(rows), "result_sha256": hashlib.sha256(encoded).hexdigest()}
    corpus_hash = hashlib.sha256(json.dumps(EXTENDED_QUERIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    manifest = {
        "schema_version": 1,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "generator": "engine/qualification/aks_cloud_fixture.py",
        "dataset": {"rows": args.rows, "customers": args.customers, "compression": "snappy",
                    "row_group_rows": 16_384, "account": args.account, "container": args.container,
                    "prefix": args.prefix, "objects": objects},
        "query_corpus": {"name": "extended", "sha256": corpus_hash, "queries": expected},
    }
    manifest_bytes = json.dumps(manifest, indent=2).encode() + b"\n"
    manifest["manifest_payload_sha256"] = hashlib.sha256(manifest_bytes).hexdigest()
    manifest_path = args.output / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    config_map = {"apiVersion": "v1", "kind": "ConfigMap",
                  "metadata": {"name": "kaveon-trino-benchmark-manifest", "namespace": "kaveon"},
                  "data": {"manifest.json": manifest_path.read_text(encoding="utf-8")}}
    (args.output / "manifest-configmap.json").write_text(json.dumps(config_map) + "\n", encoding="utf-8")

    azure = "az.cmd" if os.name == "nt" else "az"
    token = subprocess.check_output([azure, "account", "get-access-token", "--resource", "https://storage.azure.com/",
                                     "--query", "accessToken", "-o", "tsv"], text=True).strip()
    bundle = args.output / "bundle"
    bundle.mkdir()
    commands = ["#!/bin/sh", "set -eu", "cd /tmp/kaveon-benchmark-upload"]
    for index, item in enumerate(objects):
        source = data / Path(item["path"]).relative_to(args.prefix)
        target = bundle / "payload" / source.relative_to(data)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(source.read_bytes())
        url_path = quote(item["path"], safe="/")
        config = bundle / f"upload-{index}.curl"
        config.write_text("\n".join([
            f'url = "https://{args.account}.blob.core.windows.net/{args.container}/{url_path}"',
            "fail-with-body", "silent", "show-error", "max-time = 900", "request = PUT",
            f'header = "Authorization: Bearer {token}"', 'header = "x-ms-version: 2023-11-03"',
            'header = "x-ms-blob-type: BlockBlob"', 'header = "If-None-Match: *"',
            f'header = "Content-MD5: {item["content_md5"]}"',
            f'header = "x-ms-meta-sha256: {item["sha256"]}"',
            f'upload-file = "payload/{source.relative_to(data).as_posix()}"',
        ]) + "\n", encoding="utf-8", newline="\n")
        commands.append(f"curl --config upload-{index}.curl")
        commands.append(f"printf 'UPLOADED {item['path']} {item['sha256']}\\n'")
    (bundle / "run.sh").write_text("\n".join(commands) + "\n", encoding="utf-8", newline="\n")
    with tarfile.open(args.output / "upload-bundle.tar.gz", "w:gz") as archive:
        for path in sorted(bundle.rglob("*")):
            if path.is_file():
                archive.add(path, arcname="kaveon-benchmark-upload/" + path.relative_to(bundle).as_posix(), recursive=False)
    print(f"fixture={args.output / 'manifest.json'}; objects={len(objects)}; rows={args.rows}; upload_bundle_private=true")


if __name__ == "__main__":
    raise SystemExit(main())
