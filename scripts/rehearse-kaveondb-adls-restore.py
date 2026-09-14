"""Run or clean up a create-only KaveonDB ADLS restore rehearsal."""

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services.adls_artifact_client import AzureArtifactClient  # noqa: E402
from services import kaveondb_restore_rehearsal as rehearsal  # noqa: E402

MAX_MANIFEST_BYTES = 16 * 1024 * 1024


def _load(path):
    if not path.is_file() or path.stat().st_size > MAX_MANIFEST_BYTES:
        raise RuntimeError("rehearsal manifest is missing or oversized")
    try: return json.loads(path.read_text(encoding="utf-8"))
    except (OSError,ValueError) as error: raise RuntimeError("rehearsal manifest is invalid") from error


def _client(prefix):
    account,container,_=rehearsal.parse_prefix(prefix.replace(".dfs.",".blob."),
        "backups" if "/backups/" in prefix else "restores")
    return AzureArtifactClient(account,container)


def _atomic(path,value):
    path.parent.mkdir(parents=True,exist_ok=True)
    temporary=path.with_name(path.name+f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value,sort_keys=True,separators=(",",":"))+"\n",encoding="utf-8")
    os.replace(temporary,path)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest="command",required=True)
    restore=commands.add_parser("restore")
    restore.add_argument("--manifest",required=True,type=Path)
    restore.add_argument("--restore-prefix",required=True)
    restore.add_argument("--cleanup-manifest",required=True,type=Path)
    clean=commands.add_parser("cleanup")
    clean.add_argument("--cleanup-manifest",required=True,type=Path)
    args=parser.parse_args()
    try:
        if args.command=="restore":
            manifest=_load(args.manifest)
            result=rehearsal.execute(manifest,args.restore_prefix,
                _client(manifest["immutable_prefix"]),_client(args.restore_prefix))
            _atomic(args.cleanup_manifest,result.pop("cleanup_manifest"))
        else:
            cleanup_manifest=_load(args.cleanup_manifest)
            result=rehearsal.cleanup(cleanup_manifest,_client(cleanup_manifest["restore_prefix"]))
        print(json.dumps(result,sort_keys=True,separators=(",",":")))
        return 0
    except (OSError,RuntimeError,ValueError,KeyError) as error:
        print(json.dumps({"restore_executed":False,"error":str(error)},sort_keys=True,separators=(",",":")))
        return 1


if __name__=="__main__": raise SystemExit(main())
