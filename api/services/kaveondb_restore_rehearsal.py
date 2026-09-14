"""Execute and verify a create-only KaveonDB restore rehearsal."""

import hashlib
import json
from urllib.parse import urlparse

from services import kaveondb_recovery_evidence as evidence

MAX_OBJECT_BYTES = 256 * 1024 * 1024
MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
INVENTORY_PATH = "state-inventory.json"


def parse_prefix(value: str, required_segment: str) -> tuple[str, str, str]:
    parsed = urlparse(value)
    suffix = ".blob.core.windows.net"
    if (parsed.scheme != "https" or not parsed.hostname or not parsed.hostname.endswith(suffix)
            or parsed.query or parsed.fragment):
        raise RuntimeError("ADLS restore prefix is invalid")
    account = parsed.hostname[:-len(suffix)]
    parts = parsed.path.strip("/").split("/", 1)
    if len(parts) != 2 or f"/{required_segment}/" not in "/" + parts[1].rstrip("/") + "/":
        raise RuntimeError("ADLS restore prefix is invalid")
    return account, parts[0], parts[1].rstrip("/")


def execute(manifest: dict, restore_prefix: str, source_client, destination_client) -> dict:
    verified = evidence.validate_backup_manifest(manifest)
    _, _, source_root = parse_prefix(manifest["immutable_prefix"].replace(".dfs.", ".blob."), "backups")
    _, _, restore_root = parse_prefix(restore_prefix.replace(".dfs.", ".blob."), "restores")
    if source_root == restore_root:
        raise RuntimeError("restore prefix must differ from backup prefix")
    total = sum(item["size"] for item in manifest["objects"])
    if total > MAX_TOTAL_BYTES or any(item["size"] > MAX_OBJECT_BYTES for item in manifest["objects"]):
        raise RuntimeError("restore rehearsal exceeds its byte bound")
    restored, inventory = [], None
    for item in manifest["objects"]:
        content = source_client.read(f"{source_root}/{item['path']}", item["size"])
        if content is None or len(content) != item["size"] or hashlib.sha256(content).hexdigest() != item["sha256"]:
            raise RuntimeError("backup object failed source verification")
        destination = f"{restore_root}/{item['path']}"
        etag = destination_client.create_if_absent_with_etag(destination, content)
        confirmed = destination_client.read(destination, item["size"])
        if confirmed != content:
            raise RuntimeError("restored object failed read-after-write verification")
        restored.append({"path": item["path"], "etag": etag, "sha256": item["sha256"], "size": item["size"]})
        if item["path"] == INVENTORY_PATH:
            try: inventory = json.loads(content)
            except (UnicodeDecodeError, ValueError) as error: raise RuntimeError("restored state inventory is invalid") from error
    if inventory is None:
        raise RuntimeError("backup omits the state inventory")
    identity = evidence.state_identity(inventory)
    if identity["state_sha256"] != verified["state_sha256"] or identity["record_count"] != verified["record_count"]:
        raise RuntimeError("restored state identity does not match its backup manifest")
    cleanup = {"schema_version":1,"restore_prefix":restore_prefix,"objects":restored}
    return {"backup_id":verified["backup_id"],"backup_sha256":verified["manifest_sha256"],
            "restore_job_id":restore_root.rsplit("/",1)[-1],"source_inventory_sha256":identity["state_sha256"],
            "restored_inventory_sha256":identity["state_sha256"],"restored_table_count":identity["record_count"],
            "immutable_prefix":verified["immutable_prefix"],"manifest_sha256":verified["manifest_sha256"],
            "restore_executed":True,"cleanup_manifest":cleanup}


def cleanup(cleanup_manifest: dict, destination_client) -> dict:
    if (not isinstance(cleanup_manifest,dict) or set(cleanup_manifest)!={"schema_version","restore_prefix","objects"}
            or cleanup_manifest["schema_version"]!=1):
        raise RuntimeError("restore cleanup manifest is invalid")
    _,_,root=parse_prefix(cleanup_manifest["restore_prefix"].replace(".dfs.",".blob."),"restores")
    objects=cleanup_manifest["objects"]
    if not isinstance(objects,list) or not objects or len(objects)>evidence.MAX_RECORDS:
        raise RuntimeError("restore cleanup manifest is invalid")
    for item in reversed(objects):
        if (not isinstance(item,dict) or set(item)!={"path","etag","sha256","size"}
                or not isinstance(item["path"],str) or not item["path"] or item["path"].startswith(("/","http"))
                or ".." in item["path"].split("/") or not isinstance(item["etag"],str) or not item["etag"]
                or type(item["size"]) is not int or item["size"] < 0):
            raise RuntimeError("restore cleanup object is invalid")
        evidence._digest(item["sha256"],"cleanup object")
        destination_client.delete_if_match(f"{root}/{item['path']}",item["etag"])
    return {"cleanup_executed":True,"deleted_object_count":len(objects)}
