"""Prepare the private AKS upload/catalog bundle for kavedb bronze/silver/gold.

Uses deterministic synthetic fixtures; no credential is printed. Execute the
archive only in the authorized test cluster, and keep its directory private.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import subprocess
import tarfile
import pyarrow as pa
import pyarrow.parquet as pq

parser = argparse.ArgumentParser()
parser.add_argument('--account', required=True)
parser.add_argument('--private', type=Path, default=Path('tmp/aks-private-v2'))
parser.add_argument('--fixture', type=Path, default=Path('tmp/aks-medallion'))
args = parser.parse_args()
if not args.account.isalnum():
    raise SystemExit('Invalid storage account name')
out = args.private / 'kavedb-bundle'
out.mkdir(parents=True, exist_ok=True)
tokens = json.loads((args.private / 'tokens.json').read_text())
azure = 'az.cmd' if os.name == 'nt' else 'az'
upload_token = subprocess.check_output([azure, 'account', 'get-access-token', '--resource',
    'https://storage.azure.com/', '--query', 'accessToken', '-o', 'tsv'], text=True).strip()
commands = ['#!/bin/sh', 'set -eu', 'cd /tmp/kavedb-bundle']
files = set()

def request(name, url, token, method='GET', body=None, upload=None, headers=None):
    config = [f'url = {json.dumps(url)}', 'silent', 'show-error', 'max-time = 120',
        f'request = "{method}"', f'header = "Authorization: Bearer {token}"']
    if url.startswith('https://localhost:8080/'):
        config.append('header = "x-kaveon-actor: prproddu-test"')
    for key, value in (headers or {}).items():
        config.append(f'header = {json.dumps(key + ": " + value)}')
    if body is not None:
        path = out / f'{name}.json'
        path.write_text(json.dumps(body), encoding='utf-8')
        files.add(path)
        config += ['header = "Content-Type: application/json"', f'data-binary = "@{name}.json"']
    if upload:
        config += [f'upload-file = {json.dumps(upload)}']
    path = out / f'{name}.curl'
    path.write_text('\n'.join(config) + '\n', encoding='utf-8')
    files.add(path)
    return f'curl --config {name}.curl'

base = f'https://{args.account}.blob.core.windows.net/kavedb'
create = request('container', base + '?restype=container', upload_token, 'PUT', headers={'x-ms-version':'2023-11-03', 'Content-Length':'0'})
commands += [f'code=$({create} --output container.response --write-out "%{{http_code}}")',
    'case "$code" in 201|409) ;; *) echo "Container create failed: $code"; exit 1;; esac']

paths = []
for layer, name in [('bronze','orders'), ('bronze','customers'), ('silver','orders'), ('silver','customers'), ('gold','daily_sales')]:
    rel = f'{layer}/{name}/part-00000.parquet'
    path = out / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if layer == 'bronze':
        with (args.fixture / layer / name / 'part-00000.csv').open(encoding='utf-8', newline='') as stream:
            rows = list(csv.DictReader(stream))
        table = pa.Table.from_pylist(rows, schema=pa.schema([(key, pa.string()) for key in rows[0]]))
        pq.write_table(table, path, compression='snappy', row_group_size=2048)
    else:
        path.write_bytes((args.fixture / rel).read_bytes())
    files.add(path)
    paths.append((layer, name, rel, pq.read_schema(path)))
    upload = request(f'upload-{layer}-{name}', base + '/' + rel, upload_token, 'PUT', upload=rel,
        headers={'x-ms-version':'2023-11-03', 'x-ms-blob-type':'BlockBlob'})
    commands.append(upload + ' --fail --output /dev/null')

engine = 'https://localhost:8080'
def definition(label, collection, item, body):
    check = request(label+'-get', engine+item, tokens['catalog'])
    create = request(label+'-create', engine+collection, tokens['catalog'], 'POST', body)
    active = request(label+'-activate', engine+item, tokens['catalog'], 'PUT', dict(body, revision=2, lifecycle='Active'), headers={'If-Match':'1'})
    commands.extend([f'code=$({check} --output {label}.response --write-out "%{{http_code}}")',
        'if [ "$code" = 404 ]; then', create+' --fail --output /dev/null', active+' --fail --output /dev/null',
        'elif [ "$code" != 200 ]; then echo "Catalog lookup failed: $code"; exit 1; fi'])

catalog_id='aks-kavedb'
definition('catalog', '/v1/catalog/definitions', '/v1/catalog/definitions/'+catalog_id,
    {'id':catalog_id, 'name':'kavedb', 'revision':1, 'adapter':'Native',
     'storage':{'AdlsGen2':{'account':args.account,'container':'kavedb','root_path':''}},
     'credential':{'kind':'WorkloadIdentity','reference':'kaveon-test-reader'}, 'lifecycle':'Draft'})
for layer in ['bronze','silver','gold']:
    schema_id = 'aks-kavedb-'+layer
    definition(layer, f'/v1/catalog/definitions/{catalog_id}/schemas', '/v1/catalog/schemas/'+schema_id,
        {'id':schema_id,'catalog_id':catalog_id,'name':layer,'revision':1,'lifecycle':'Draft'})
for layer,name,rel,schema in paths:
    table_id=f'aks-kavedb-{layer}-{name}'
    columns=[{'name':f.name,'data_type':{'int64':'Int64','string':'Utf8'}[str(f.type)],'nullable':f.nullable} for f in schema]
    definition(layer+'-'+name, f'/v1/catalog/schemas/aks-kavedb-{layer}/tables', '/v1/catalog/tables/'+table_id,
        {'id':table_id,'schema_id':'aks-kavedb-'+layer,'name':name,'revision':1,'location':rel,
         'access':'Shortcut','format':'Parquet','columns':columns,'lifecycle':'Draft'})
commands.append('echo "Prepared kavedb: bronze.orders/customers, silver.orders/customers, gold.daily_sales"')
path=out/'run.sh';path.write_text('\n'.join(commands)+'\n',encoding='utf-8',newline='\n');files.add(path)
with tarfile.open(args.private/'kavedb-bundle.tar.gz','w:gz') as archive:
    for path in sorted(files):
        archive.add(path,arcname='kavedb-bundle/'+path.relative_to(out).as_posix())
print('Prepared private kavedb bundle. Credential files must not be printed or committed.')
