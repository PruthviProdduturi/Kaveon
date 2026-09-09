"""Download one immutable OpenSource ADLS snapshot to a verified local cache.

Authentication is obtained from the current Azure CLI session and remains only
in this process. The script never writes or prints the token.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tarfile
import tempfile
from datetime import datetime, timezone
from email.utils import format_datetime
from pathlib import Path, PurePosixPath
from urllib.parse import quote
from xml.etree import ElementTree

import requests


DEFAULT_ACCOUNT = "kvtestegmf6oweugsno"
DEFAULT_CONTAINER = "opensource"
DEFAULT_PREFIX = "snapshots/2026-09-09-v1"
DEFAULT_DESTINATION = Path("data/opensource/2026-09-09-v1")
API_VERSION = "2023-11-03"

# Runs inside the existing API Pod. stdout is exclusively a tar stream; the
# caller token is read once from stdin and never appears in an argument, file,
# manifest, stderr, or archive.
AKS_RELAY = r'''
import hashlib, io, json, sys, tarfile
from datetime import datetime, timezone
from email.utils import format_datetime
from urllib.parse import quote
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from xml.etree import ElementTree

account, container, prefix = sys.argv[1:4]
token = sys.stdin.buffer.readline().strip().decode("ascii")
if not token:
    raise RuntimeError("missing storage token")
def headers():
    return {"Authorization": "Bearer " + token, "x-ms-date": format_datetime(datetime.now(timezone.utc), usegmt=True), "x-ms-version": "2023-11-03"}
def url(name=""):
    root = "https://" + account + ".blob.core.windows.net/" + quote(container, safe="")
    return root if not name else root + "/" + quote(name, safe="/")
def fetch(request):
    return urlopen(request, timeout=180)
def safe(name):
    base = prefix.rstrip("/") + "/"
    if not name.startswith(base): raise RuntimeError("unsafe remote path")
    value = name[len(base):]
    if not value or value.startswith("/") or ".." in value.split("/") or "\\" in value or ":" in value: raise RuntimeError("unsafe remote path")
    return value
def blobs():
    marker = None
    while True:
        query = "?restype=container&comp=list&include=metadata&prefix=" + quote(prefix.rstrip("/") + "/", safe="/")
        if marker: query += "&marker=" + quote(marker, safe="")
        with fetch(Request(url() + query, headers=headers())) as response:
            root = ElementTree.fromstring(response.read())
        for node in root.findall(".//Blobs/Blob"):
            name = node.findtext("Name")
            if name:
                metadata = {child.tag.lower(): child.text or "" for child in node.findall("./Metadata/*")}
                size = int(node.findtext("./Properties/Content-Length") or "0")
                yield name, metadata.get("sha256", "").lower(), size
        marker = root.findtext(".//NextMarker") or None
        if not marker: return
class HashReader:
    def __init__(self, stream): self.stream, self.digest, self.size = stream, hashlib.sha256(), 0
    def read(self, size=-1):
        value = self.stream.read(size)
        if value: self.digest.update(value); self.size += len(value)
        return value
try:
    entries = []
    stage = "opening archive"
    with tarfile.open(fileobj=sys.stdout.buffer, mode="w|") as archive:
        for name, expected, size in blobs():
            stage = "reading blob metadata"
            relative = safe(name)
            # ADLS Gen2 exposes empty directory markers alongside files.
            # They have no content checksum and are not snapshot content.
            if not expected and size == 0:
                continue
            if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
                raise RuntimeError("remote blob metadata lacks SHA-256: " + relative)
            stage = "streaming blob"
            with fetch(Request(url(name), headers=headers())) as response:
                reader = HashReader(response)
                info = tarfile.TarInfo(relative); info.size = size; info.mode = 0o600
                archive.addfile(info, reader)
            if reader.size != size or reader.digest.hexdigest() != expected:
                raise RuntimeError("remote blob checksum mismatch")
            entries.append({"path": relative, "sha256": expected, "bytes": size})
        payload = json.dumps({"account": account, "container": container, "prefix": prefix.rstrip("/"), "files": entries}, sort_keys=True).encode("utf-8")
        info = tarfile.TarInfo("inventory.json"); info.size = len(payload); info.mode = 0o600
        archive.addfile(info, io.BytesIO(payload))
    print("AKS relay prepared %d blobs" % len(entries), file=sys.stderr)
except HTTPError as error:
    print("AKS snapshot relay failed during " + stage + " with HTTP " + str(error.code), file=sys.stderr)
    raise SystemExit(1)
except RuntimeError as error:
    print("AKS snapshot relay failed during " + stage + ": " + str(error), file=sys.stderr)
    raise SystemExit(1)
except Exception:
    print("AKS snapshot relay failed during " + stage, file=sys.stderr)
    raise SystemExit(1)
'''


def azure_token() -> str:
    """Return an Azure Storage bearer token without exposing it to shell output."""
    executable = "az.cmd" if os.name == "nt" else "az"
    result = subprocess.run(
        [executable, "account", "get-access-token", "--resource", "https://storage.azure.com/", "--query", "accessToken", "--output", "tsv"],
        check=True, capture_output=True, text=True,
    )
    token = result.stdout.strip()
    if not token:
        raise RuntimeError("Azure CLI returned an empty storage access token")
    return token


def headers(token: str) -> dict[str, str]:
    return {
        "Authorization": f"Bearer {token}",
        "x-ms-date": format_datetime(datetime.now(timezone.utc), usegmt=True),
        "x-ms-version": API_VERSION,
    }


def blob_url(account: str, container: str, name: str = "") -> str:
    root = f"https://{account}.blob.core.windows.net/{quote(container, safe='')}"
    return root if not name else root + "/" + quote(name, safe="/")


def response_or_error(response: requests.Response, action: str) -> None:
    if response.ok:
        return
    # Azure's body can contain request information; do not surface it because a
    # future authentication failure must not risk printing credential material.
    raise RuntimeError(f"Azure Blob Storage {action} failed with HTTP {response.status_code}")


def list_blobs(account: str, container: str, prefix: str, token: str) -> list[dict]:
    blobs, marker = [], None
    while True:
        params = {"restype": "container", "comp": "list", "prefix": prefix.rstrip("/") + "/", "include": "metadata"}
        if marker:
            params["marker"] = marker
        response = requests.get(blob_url(account, container), params=params, headers=headers(token), timeout=(15, 90))
        response_or_error(response, "listing")
        root = ElementTree.fromstring(response.content)
        for node in root.findall(".//Blobs/Blob"):
            name = node.findtext("Name")
            if not name:
                continue
            metadata = {child.tag.lower(): child.text or "" for child in node.findall("./Metadata/*")}
            blobs.append({"name": name, "metadata": metadata,
                          "bytes": int(node.findtext("./Properties/Content-Length") or "0")})
        marker = root.findtext(".//NextMarker") or None
        if not marker:
            break
    return blobs


def relative_blob_path(name: str, prefix: str) -> Path:
    required = prefix.rstrip("/") + "/"
    if not name.startswith(required):
        raise RuntimeError("listed blob is outside the requested snapshot prefix")
    relative = PurePosixPath(name[len(required):])
    if (
        not relative.parts
        or relative.is_absolute()
        or ".." in relative.parts
        or "\\" in name
        or ":" in name
    ):
        raise RuntimeError("snapshot contains an unsafe blob path")
    return Path(*relative.parts)


def cache_target(destination: Path, relative: Path) -> Path:
    root = destination.resolve()
    target = (root / relative).resolve()
    if not target.is_relative_to(root):
        raise RuntimeError("snapshot path escapes the local cache")
    return target


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def expected_hash(account: str, container: str, name: str, token: str) -> tuple[str, int]:
    response = requests.head(blob_url(account, container, name), headers=headers(token), timeout=(15, 60))
    response_or_error(response, "reading blob metadata")
    digest = response.headers.get("x-ms-meta-sha256")
    if not digest or len(digest) != 64 or any(char not in "0123456789abcdefABCDEF" for char in digest):
        raise RuntimeError(f"blob '{name}' has no valid x-ms-meta-sha256")
    return digest.lower(), int(response.headers.get("Content-Length", "0"))


def download_blob(account: str, container: str, name: str, target: Path, token: str, expected: str) -> int:
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=target.name + ".", suffix=".partial", dir=target.parent)
    digest, size = hashlib.sha256(), 0
    try:
        with os.fdopen(descriptor, "wb") as stream:
            response = requests.get(blob_url(account, container, name), headers=headers(token), stream=True, timeout=(15, 180))
            response_or_error(response, "downloading")
            for block in response.iter_content(chunk_size=1024 * 1024):
                if block:
                    stream.write(block)
                    digest.update(block)
                    size += len(block)
        actual = digest.hexdigest()
        if actual != expected:
            raise RuntimeError(f"checksum mismatch for '{name}'")
        if target.exists():
            # Never replace an independently created/different local file.
            if sha256(target) != expected:
                raise RuntimeError(f"local file differs and was not overwritten: {target}")
            return target.stat().st_size
        os.replace(temporary_name, target)
        temporary_name = ""
        return size
    finally:
        if temporary_name:
            Path(temporary_name).unlink(missing_ok=True)


def kubectl_name() -> str:
    bundled = Path("tmp/aks-tools") / ("kubectl.exe" if os.name == "nt" else "kubectl")
    return str(bundled) if bundled.is_file() else ("kubectl.exe" if os.name == "nt" else "kubectl")


def kubectl_environment() -> dict[str, str]:
    """Make the repository's AKS kubelogin helper available to kubectl."""
    environment = os.environ.copy()
    tools = Path("tmp/aks-tools").resolve()
    if tools.is_dir():
        environment["PATH"] = str(tools) + os.pathsep + environment.get("PATH", "")
    return environment


def api_pod(kubeconfig: Path, namespace: str, selector: str) -> str:
    result = subprocess.run(
        [kubectl_name(), "--kubeconfig", str(kubeconfig), "-n", namespace, "get", "pods", "-l", selector,
         "-o", "jsonpath={.items[0].metadata.name}"],
        check=True, capture_output=True, text=True, env=kubectl_environment(),
    )
    pod = result.stdout.strip()
    if not pod:
        raise RuntimeError("no API pod matched the requested selector")
    return pod


def safe_member_path(value: str) -> Path:
    path = PurePosixPath(value)
    if (
        not path.parts
        or path.is_absolute()
        or ".." in path.parts
        or value == "inventory.json"
        or "\\" in value
        or ":" in value
    ):
        raise RuntimeError("AKS relay archive contains an unsafe path")
    return Path(*path.parts)


def extract_relay_archive(archive_path: Path, destination: Path) -> tuple[list[dict], int, int]:
    """Verify every relayed member before atomically placing it in the cache."""
    with tarfile.open(archive_path, "r:") as archive:
        member = archive.getmember("inventory.json")
        source = archive.extractfile(member)
        if source is None:
            raise RuntimeError("AKS relay archive lacks inventory")
        inventory = json.loads(source.read().decode("utf-8"))
        files = inventory.get("files")
        if not isinstance(files, list) or not files:
            raise RuntimeError("AKS relay inventory is invalid")
        listed = {entry.get("path") for entry in files if isinstance(entry, dict)}
        archive_files = {item.name for item in archive.getmembers() if item.isfile() and item.name != "inventory.json"}
        if listed != archive_files:
            raise RuntimeError("AKS relay inventory does not match archive members")
        stage = Path(tempfile.mkdtemp(prefix="opensource-extract-", dir=destination.parent))
        downloaded = skipped = 0
        try:
            for entry in files:
                relative = safe_member_path(entry["path"])
                expected, size = entry.get("sha256"), entry.get("bytes")
                if not isinstance(expected, str) or len(expected) != 64 or not isinstance(size, int):
                    raise RuntimeError("AKS relay inventory entry is invalid")
                target = cache_target(destination, relative)
                if target.exists() and sha256(target) == expected:
                    if target.stat().st_size != size:
                        raise RuntimeError(f"cached file byte count differs: {target}")
                    skipped += 1
                    continue
                source = archive.extractfile(entry["path"])
                if source is None:
                    raise RuntimeError("AKS relay archive member is unavailable")
                candidate = stage / relative
                candidate.parent.mkdir(parents=True, exist_ok=True)
                with candidate.open("wb") as stream:
                    for block in iter(lambda: source.read(1024 * 1024), b""):
                        stream.write(block)
                if candidate.stat().st_size != size or sha256(candidate) != expected:
                    raise RuntimeError(f"relayed file verification failed: {relative}")
                if target.exists() and sha256(target) != expected:
                    raise RuntimeError(f"local file differs and was not overwritten: {target}")
                target.parent.mkdir(parents=True, exist_ok=True)
                os.replace(candidate, target)
                downloaded += 1
        finally:
            import shutil
            shutil.rmtree(stage, ignore_errors=True)
    return files, downloaded, skipped


def download_via_aks(args, token: str, destination: Path) -> tuple[list[dict], int, int]:
    pod = api_pod(args.kubeconfig, args.namespace, args.pod_selector)
    descriptor, archive_name = tempfile.mkstemp(prefix="opensource-relay-", suffix=".tar", dir=destination.parent)
    try:
        command = [kubectl_name(), "--kubeconfig", str(args.kubeconfig), "-n", args.namespace, "exec", "-i", pod,
                   "--", "python", "-c", AKS_RELAY, args.account, args.container, args.prefix]
        with os.fdopen(descriptor, "wb") as output:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=output, stderr=subprocess.PIPE,
                                       env=kubectl_environment())
            _, stderr = process.communicate((token + "\n").encode("ascii"))
        if process.returncode:
            summary = stderr.decode("utf-8", "replace").strip()
            raise RuntimeError(summary or "AKS snapshot relay failed")
        summary = stderr.decode("utf-8", "replace").strip()
        if summary:
            print(summary)
        return extract_relay_archive(Path(archive_name), destination)
    finally:
        Path(archive_name).unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", default=DEFAULT_ACCOUNT)
    parser.add_argument("--container", default=DEFAULT_CONTAINER)
    parser.add_argument("--prefix", default=DEFAULT_PREFIX)
    parser.add_argument("--destination", type=Path, default=DEFAULT_DESTINATION)
    parser.add_argument("--via-aks", action="store_true", help="relay through the existing API pod when local storage networking is blocked")
    parser.add_argument("--kubeconfig", type=Path, default=Path("tmp/aks-kubeconfig"))
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--pod-selector", default="app=kaveon-api")
    args = parser.parse_args()
    token = azure_token()
    destination = args.destination.resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    if args.via_aks:
        inventory, downloaded, skipped = download_via_aks(args, token, destination)
        payload = {
            "account": args.account,
            "container": args.container,
            "prefix": args.prefix.rstrip("/"),
            "files": sorted(inventory, key=lambda item: item["path"]),
        }
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "inventory.json").write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"Verified {len(inventory)} blobs via AKS: downloaded {downloaded}, already cached {skipped}; inventory: {destination / 'inventory.json'}")
        return
    blobs = list_blobs(args.account, args.container, args.prefix, token)
    if not blobs:
        raise RuntimeError("no blobs found under the requested snapshot prefix")

    inventory, downloaded, skipped = [], 0, 0
    for blob in blobs:
        name = blob["name"]
        expected, remote_bytes = expected_hash(args.account, args.container, name, token)
        target = cache_target(destination, relative_blob_path(name, args.prefix))
        if target.exists() and sha256(target) == expected:
            size = target.stat().st_size
            skipped += 1
        else:
            size = download_blob(args.account, args.container, name, target, token, expected)
            downloaded += 1
        if size != remote_bytes:
            raise RuntimeError(f"byte count mismatch for '{name}'")
        inventory.append({"path": target.relative_to(destination).as_posix(), "sha256": expected, "bytes": size})

    destination.mkdir(parents=True, exist_ok=True)
    payload = {
        "account": args.account,
        "container": args.container,
        "prefix": args.prefix.rstrip("/"),
        "files": sorted(inventory, key=lambda item: item["path"]),
    }
    (destination / "inventory.json").write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Verified {len(inventory)} blobs: downloaded {downloaded}, already cached {skipped}; inventory: {destination / 'inventory.json'}")


if __name__ == "__main__":
    main()
