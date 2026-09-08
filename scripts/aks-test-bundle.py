"""Prepare a private curl validation bundle for AKS command invoke.

No tokens appear in shell arguments or output. The bundle contains short-lived
credentials and must stay in a private ignored directory; never commit it.
"""
import argparse
import json
import os
import re
from pathlib import Path
import subprocess
import tarfile
import pyarrow.parquet as pq

p = argparse.ArgumentParser()
p.add_argument('--account', required=True)
p.add_argument('--private', type=Path, default=Path('tmp/aks-private-v2'))
p.add_argument('--fixture', type=Path, default=Path('tmp/aks-medallion'))
mode = p.add_mutually_exclusive_group()
mode.add_argument('--queries-only', action='store_true', help='Recheck an already bootstrapped catalog')
mode.add_argument('--repair-initial-catalog', action='store_true', help='Repair initial revision-2 tables and add gold catalog; run once')
args = p.parse_args()
out = args.private / 'bundle'
out.mkdir(exist_ok=True)
tokens = json.loads((args.private / 'tokens.json').read_text())
azure = 'az.cmd' if os.name == 'nt' else 'az'
upload_token = None
if not (args.queries_only or args.repair_initial_catalog):
    upload_token = subprocess.check_output([azure, 'account', 'get-access-token', '--resource', 'https://storage.azure.com/', '--query', 'accessToken', '-o', 'tsv'], text=True).strip()
commands = ['#!/bin/sh', 'set -eu', 'cd /tmp/aks-bundle']
bundle_files = set()
def curl_config(name, url, headers, method=None, body=None, upload=None, report=False):
    if args.queries_only and not report:
        return
    lines = [f'url = {json.dumps(url)}', 'fail-with-body', 'silent', 'show-error', 'max-time = 120']
    lines += [f'header = {json.dumps(k + ": " + v)}' for k, v in headers.items()]
    if method:
        lines.append(f'request = "{method}"')
    if body is not None:
        (out / (name + '.json')).write_text(json.dumps(body), encoding='utf-8')
        bundle_files.add(out / (name + '.json'))
        lines.append(f'data-binary = "@{name}.json"')
    if upload:
        lines.append(f'upload-file = "{upload}"')
    (out / (name + '.curl')).write_text('\n'.join(lines) + '\n', encoding='utf-8')
    bundle_files.add(out / (name + '.curl'))
    if report:
        commands.append(f'printf "RESULT {name} "')
        commands.append(f'curl --config {name}.curl')
        commands.append("printf '\\n'")
    else:
        commands.append(f'curl --config {name}.curl > {name}.response')

for layer in ([] if upload_token is None else ['bronze', 'silver', 'gold']):
    for index, path in enumerate(sorted((args.fixture / layer).rglob('*'))):
        if path.is_file():
            rel = path.relative_to(args.fixture).as_posix()
            target = out / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(path.read_bytes())
            bundle_files.add(target)
            curl_config(f'upload-{layer}-{index}', f'https://{args.account}.blob.core.windows.net/{rel}',
                        {'Authorization': 'Bearer ' + upload_token, 'x-ms-version': '2023-11-03', 'x-ms-blob-type': 'BlockBlob'}, upload=rel)

base = 'https://localhost:8080'
def definition(name, collection, item, value):
    headers = {'Authorization': 'Bearer ' + tokens['catalog'], 'Content-Type': 'application/json', 'x-kaveon-actor': 'prproddu-test'}
    curl_config(name + '-create', base + collection, headers, 'POST', value)
    curl_config(name + '-activate', base + item, dict(headers, **{'If-Match': '1'}), 'PUT', dict(value, revision=2, lifecycle='Active'))

for layer, catalog_id, catalog_name, schema_id in [
    ('silver', 'aks-medallion', 'medallion', 'aks-test'),
    ('gold', 'aks-medallion-gold', 'medallion_gold', 'aks-test-gold'),
]:
    if args.repair_initial_catalog and layer == 'silver':
        continue
    definition(layer + '-catalog', '/v1/catalog/definitions', '/v1/catalog/definitions/' + catalog_id, {
        'id': catalog_id, 'name': catalog_name, 'revision': 1, 'adapter': 'Native',
        'storage': {'AdlsGen2': {'account': args.account, 'container': layer, 'root_path': ''}},
        'credential': {'kind': 'WorkloadIdentity', 'reference': 'kaveon-test-reader'}, 'lifecycle': 'Draft'})
    definition(layer + '-schema', '/v1/catalog/definitions/' + catalog_id + '/schemas', '/v1/catalog/schemas/' + schema_id, {
        'id': schema_id, 'catalog_id': catalog_id, 'name': 'test', 'revision': 1, 'lifecycle': 'Draft'})
for layer, table in [('silver', 'orders'), ('silver', 'customers'), ('gold', 'daily_sales')]:
    schema = pq.read_schema(args.fixture / layer / table / 'part-00000.parquet')
    columns = [{'name': f.name, 'data_type': {'int64': 'Int64', 'string': 'Utf8'}[str(f.type)], 'nullable': f.nullable} for f in schema]
    table_id = 'aks-' + table if layer == 'silver' else 'aks-gold-' + table
    schema_id = 'aks-test' if layer == 'silver' else 'aks-test-gold'
    value = {'id': table_id, 'schema_id': schema_id, 'name': table, 'revision': 1,
             'location': f'{table}/part-00000.parquet', 'access': 'Shortcut',
             'format': 'Parquet', 'columns': columns, 'lifecycle': 'Draft'}
    repair_headers = {'Authorization': 'Bearer ' + tokens['catalog'], 'Content-Type': 'application/json',
                      'x-kaveon-actor': 'prproddu-test', 'If-Match': '2'}
    if args.repair_initial_catalog and layer == 'silver':
        curl_config(table + '-repair', base + '/v1/catalog/tables/' + table_id, repair_headers,
                    'PUT', dict(value, revision=3, lifecycle='Active'))
    else:
        if args.repair_initial_catalog:
            old = dict(value, id='aks-daily_sales', schema_id='aks-test', revision=3, lifecycle='Suspended',
                       location=f'abfss://gold@{args.account}.dfs.core.windows.net/daily_sales/part-00000.parquet')
            curl_config('daily_sales-suspend', base + '/v1/catalog/tables/aks-daily_sales', repair_headers, 'PUT', old)
        definition(table, '/v1/catalog/schemas/' + schema_id + '/tables', '/v1/catalog/tables/' + table_id, value)
headers = {'Authorization': 'Bearer ' + tokens['principal'], 'Content-Type': 'application/json'}
curl_config('nodes', base + '/v1/cluster', headers, report=True)
for case in json.loads((args.fixture / 'expected-results.json').read_text()):
    curl_config(case['name'], base + '/v1/statement', headers, 'POST',
                {'query': re.sub(r'\bdaily_sales\b', 'medallion_gold.test.daily_sales', case['sql']),
                 'catalog': 'medallion', 'schema': 'test', 'result_delivery': 'inline'}, report=True)
commands += ['printf "UNAUTHORIZED "', 'curl --silent --output /dev/null --write-out "%{http_code}" https://localhost:8080/v1/catalog', "printf '\\n'"]
(out / 'run.sh').write_text('\n'.join(commands) + '\n', encoding='utf-8', newline='\n')
bundle_files.add(out / 'run.sh')
with tarfile.open(args.private / 'aks-bundle.tar.gz', 'w:gz') as archive:
    for path in sorted(bundle_files):
        archive.add(path, arcname='aks-bundle/' + path.relative_to(out).as_posix(), recursive=False)
print('Prepared private bundle (' + ('queries only' if args.queries_only else 'one-time initial repair' if args.repair_initial_catalog else 'fresh bootstrap') + '). Do not print its credential files.')
