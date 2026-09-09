"""Consolidate immutable synthetic daily Parquet parts into readable single files."""
import io, json, os
from pathlib import Path
import pyarrow.parquet as pq
import requests

ACCOUNT=os.environ['ADLS_ACCOUNT']; CONTAINER='opensource'; PREFIX='snapshots/2026-09-09-v1/kaveon_product'
token=os.environ['ADLS_UPLOAD_TOKEN']; base=f'https://{ACCOUNT}.blob.core.windows.net/{CONTAINER}/{PREFIX}'
headers={'Authorization':'Bearer '+token,'x-ms-version':'2023-11-03'}
out=Path('/work/kaveon_product'); out.mkdir(parents=True,exist_ok=True)
def combine(name):
    target=out/name/'combined-v1.parquet'; target.parent.mkdir(parents=True,exist_ok=True)
    writer=None; rows=0; columns=None
    try:
        for day in range(230):
            url=f'{base}/{name}/part-{day:03d}.parquet'
            response=requests.get(url,headers=headers,timeout=(15,300)); response.raise_for_status()
            table=pq.read_table(io.BytesIO(response.content)); columns=table.schema
            if writer is None: writer=pq.ParquetWriter(target,columns,compression='zstd')
            writer.write_table(table); rows+=table.num_rows
    finally:
        if writer: writer.close()
    return {'schema':'kaveon_product','name':name,'location':f'kaveon_product/{name}/combined-v1.parquet','row_count':rows,'columns':[{'name':f.name,'data_type':'Float64' if str(f.type) in ('double','float') else 'Int64' if 'int' in str(f.type) else 'Utf8','nullable':True} for f in columns]}
tables=[combine('kaveon_usage_daily'),combine('kaveon_product_analytics')]
Path('/work/kaveon-product-readable-manifest.json').write_text(json.dumps({'tables':tables},indent=2),encoding='utf-8')
