"""Register previously validated ADLS table manifests from an API pod.

Requires the API's existing Engine credential and CA environment, plus
KAVEON_LAKE_ADLS_ACCOUNT (or KAVEON_LOCAL_LAKE_PATH for a local lake). Manifest
arguments are local JSON files; no credentials or data rows are printed.
"""
import json
import argparse
import os
import re
import sys
import time
from pathlib import Path
from urllib.parse import quote
import httpx
import database.metadata as metadata

ROOT = 'snapshots/2026-09-09-v1'
# Storage comes from the environment so the same registration runs on AKS
# (ADLS Gen2 through workload identity) and on the local cluster (a directory).
LOCAL_LAKE = os.environ.get('KAVEON_LOCAL_LAKE_PATH')
if LOCAL_LAKE:
    STORAGE = {'Local': {'base_path': LOCAL_LAKE}}
    CREDENTIAL = None
    SOURCE_STORAGE = ('local', json.dumps({'base_path': LOCAL_LAKE}), 'environment', 'local')
else:
    # The account is an environment fact, never a constant: the qualification
    # cluster has already been rebuilt once into a different account.
    ACCOUNT = os.environ['KAVEON_LAKE_ADLS_ACCOUNT']
    ADLS = {'account': ACCOUNT, 'container': 'opensource', 'root_path': ROOT}
    STORAGE = {'AdlsGen2': ADLS}
    CREDENTIAL = {'kind': 'WorkloadIdentity', 'reference': 'kaveon-test-reader'}
    SOURCE_STORAGE = ('adls_gen2', json.dumps(ADLS), 'workload_identity', 'kaveon-test-reader')
client = httpx.Client(base_url=os.environ['KAVEON_ENGINE_URL'],
                     verify=os.environ.get('KAVEON_ENGINE_CA_CERT') or True, timeout=120,
                     headers={'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_CATALOG_TOKEN'],
                              'x-kaveon-actor': 'catalog-curation'})

def register(collection, path, body):
    existing = client.get(path)
    if existing.status_code == 404:
        response = client.post(collection, json={**body, 'revision': 1, 'lifecycle': 'Draft'})
        response.raise_for_status()
        current = response.json()
    else:
        existing.raise_for_status()
        current = existing.json()
        for key, value in body.items():
            if current.get(key) != value:
                raise RuntimeError(f'Existing definition differs: {path}, field {key}')
    if current['lifecycle'] != 'Active':
        revision = current['revision']
        response = client.put(path, headers={'If-Match': str(revision)},
                              json={**body, 'revision': revision+1, 'lifecycle': 'Active'})
        response.raise_for_status()

register('/v1/catalog/definitions', '/v1/catalog/definitions/'+CATALOG_ID,
         {'id': CATALOG_ID, 'name': CATALOG, 'adapter': 'Native', 'storage': STORAGE,
          **({'credential': CREDENTIAL} if CREDENTIAL else {})})
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--definitions-only', action='store_true',
                    help='Register definitions without running reads or updating API metadata')
parser.add_argument('manifests', nargs='+')
args = parser.parse_args()
documents = [json.loads(Path(filename).read_text()) for filename in args.manifests]
catalog_names = {document.get('catalog', 'OpenSource') for document in documents}
if len(catalog_names) != 1:
    parser.error('register one catalog at a time; all manifests must name the same catalog')
CATALOG = catalog_names.pop()
if CATALOG not in {'OpenSource', 'Kaveon'}:
    parser.error(f'unsupported curated catalog: {CATALOG}')
CATALOG_ID = ('local-' if LOCAL_LAKE else 'aks-') + CATALOG.lower()
tables = [table for document in documents for table in document['tables']]
# Publish subject schemas; physical medallion paths remain unchanged in ADLS.
publication = {
    ('silver', 'yellow_trips'): ('nyc_taxi', 'yellow_trips'),
    ('silver', 'green_trips'): ('nyc_taxi', 'green_trips'),
    ('gold', 'daily_trips'): ('nyc_taxi', 'daily_trips'),
    ('reference', 'taxi_zones'): ('nyc_taxi', 'taxi_zones'),
    ('reference', 'energy_indicators'): ('climate_energy', 'energy_indicators'),
    ('gold', 'covid_reported_by_date'): ('covid', 'reported_by_date'),
}
excluded = {('bronze', 'yellow_trips'), ('bronze', 'green_trips'),
            ('silver', 'yellow_rejected'), ('silver', 'green_rejected')}
curated = []
for table in tables:
    key = (table['schema'], table['name'])
    if key in excluded:
        continue
    schema, name = publication.get(key, key)
    curated.append({**table, 'schema': schema, 'name': name})
tables = curated

for schema in sorted({t['schema'] for t in tables}):
    schema_id = CATALOG_ID+'-'+schema
    register(f'/v1/catalog/definitions/{CATALOG_ID}/schemas', '/v1/catalog/schemas/'+schema_id,
             {'id': schema_id, 'catalog_id': CATALOG_ID, 'name': schema})
for table in tables:
    schema_id = CATALOG_ID+'-'+table['schema']
    table_id = schema_id+'-'+table['name']
    register('/v1/catalog/schemas/'+quote(schema_id, safe='')+'/tables',
             '/v1/catalog/tables/'+quote(table_id, safe=''),
             {'id': table_id, 'schema_id': schema_id, 'name': table['name'],
              'location': table['location'], 'access': 'Shortcut', 'format': 'Parquet',
              'columns': table['columns']})

# Verify actual distributed reads before exposing the source in SQL Lab.
if args.definitions_only:
    print(json.dumps({'catalog': CATALOG, 'registered_tables': len(tables),
                      'definitions_only': True}))
    raise SystemExit(0)
checks = []
for table in tables:
    schema = table['schema']
    name = table['name']
    if not all(re.fullmatch(r'[a-z_][a-z0-9_]*', value) for value in (schema, name)):
        raise ValueError('This fixed bootstrap accepts only simple identifiers')
    response = client.post('/v1/statement', headers={
        'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_BRIDGE_TOKEN'],
        'x-kaveon-principal': CATALOG.lower() + '-curation', 'x-kaveon-role': 'admin',
    }, json={'query': f'SELECT COUNT(*) FROM {schema}.{name}',
             'catalog': CATALOG, 'schema': table['schema'], 'source': 'http-api'})
    if response.status_code >= 500:
        time.sleep(2)
        response = client.send(response.request)
    response.raise_for_status()
    result = response.json()
    rows = result.get('data', result.get('rows'))
    if result.get('error') or rows != [[table['row_count']]]:
        raise RuntimeError(f'Engine row-count mismatch for {schema}.{name}: {result.get("error", rows)}')
    checks.append({'table': schema+'.'+name, 'rows': table['row_count'], 'query_id': result.get('id')})
    print('Verified ' + schema+'.'+name + ': ' + str(table['row_count']), flush=True)
metadata.execute('''INSERT INTO catalog_sources
    (name, engine_catalog, storage_type, storage_config, data_format,
     credential_kind, credential_ref, adapter_type, adapter_config,
     lifecycle, description, created_by, modified_by)
    VALUES (@param0, @param1, @param2, @param3, 'parquet',
            @param4, @param5, 'native', '{}',
            'active', @param6, 'system', 'system')
    ON CONFLICT (engine_catalog) DO UPDATE SET
      name=EXCLUDED.name, description=EXCLUDED.description, storage_config=EXCLUDED.storage_config,
      storage_type=EXCLUDED.storage_type, credential_kind=EXCLUDED.credential_kind, credential_ref=EXCLUDED.credential_ref,
      lifecycle='active', modified_at=NOW()''',
    [CATALOG, CATALOG, SOURCE_STORAGE[0], SOURCE_STORAGE[1], SOURCE_STORAGE[2], SOURCE_STORAGE[3],
     ('Public NYC Taxi, WHO COVID, climate/energy and archived AI benchmark datasets' if CATALOG == 'OpenSource'
      else 'Kaveon synthetic usage and product showcase datasets')])
print(json.dumps({'catalog': CATALOG, 'registered_tables': len(tables)}))
print('VALIDATION=' + json.dumps(checks))
