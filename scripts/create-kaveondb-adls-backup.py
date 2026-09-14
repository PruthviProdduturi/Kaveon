"""Create and verify an immutable KaveonDB ADLS backup manifest."""
import argparse,json,os,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"api"))
from services.adls_artifact_client import AzureArtifactClient  # noqa: E402
from services import kaveondb_backup  # noqa: E402

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument("--account",required=True);parser.add_argument("--container",required=True)
 parser.add_argument("--active-prefix",required=True);parser.add_argument("--backup-id",required=True)
 parser.add_argument("--manifest-output",required=True,type=Path);args=parser.parse_args()
 try:
  client=AzureArtifactClient(args.account,args.container)
  records,state=kaveondb_backup.product_inventory()
  result=kaveondb_backup.create(args.active_prefix,args.backup_id,client,client,records,state)
  args.manifest_output.parent.mkdir(parents=True,exist_ok=True)
  temporary=args.manifest_output.with_name(args.manifest_output.name+f".{os.getpid()}.tmp")
  temporary.write_text(json.dumps(result.pop("manifest"),sort_keys=True,separators=(",",":"))+"\n",encoding="utf-8")
  os.replace(temporary,args.manifest_output)
  print(json.dumps({"backup_created":True,**result},sort_keys=True,separators=(",",":")));return 0
 except (OSError,RuntimeError,ValueError,KeyError) as error:
  print(json.dumps({"backup_created":False,"error":str(error)},sort_keys=True,separators=(",",":")));return 1
if __name__=="__main__":raise SystemExit(main())
